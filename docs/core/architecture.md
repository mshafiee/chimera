# Chimera Architecture Documentation

## Overview

Chimera is a high-frequency copy-trading system for Solana that automatically executes trades based on signals from tracked wallets. The system is designed for low-latency execution, fault tolerance, and operational reliability.

---

## System Components

### 1. Operator Service (Rust)

The core trading engine written in Rust using Tokio for async operations.

**Key Modules:**
- **Engine**: Trade execution engine with state machine
- **Executor**: RPC interaction and transaction submission
- **Recovery**: Position recovery and stuck state handling
- **Circuit Breaker**: Risk management and trading halt
- **Price Cache**: Token price caching and updates
- **Token Parser**: Token metadata and safety checks
- **Vault**: Encrypted keypair storage

### 2. Scout Service (Python)

Wallet analysis and WQS (Wallet Quality Score) calculation.

**Key Components:**
- **Analyzer**: Fetches wallet transaction history
- **WQS Calculator**: Computes wallet quality scores
- **Backtester**: Pre-promotion trade simulation
- **DB Writer**: Updates wallet roster in database

### 3. Web Dashboard (TypeScript/React)

Real-time monitoring and management interface.

**Key Features:**
- Real-time position tracking via WebSocket
- Wallet management and promotion
- Trade history and export
- Configuration management
- Performance metrics visualization

---

## Data Flow

### Signal Processing Flow

```
1. External Signal Provider
   ↓
2. Webhook Endpoint (/api/v1/webhook)
   ├─ HMAC Signature Verification
   ├─ Rate Limiting (100 req/s)
   └─ Queue Depth Check (< 1000)
   ↓
3. Signal Validation
   ├─ Token Safety Checks (honeypot, liquidity)
   ├─ Wallet Status Check (ACTIVE only)
   ├─ Circuit Breaker Check
   └─ Strategy Allocation (Shield vs Spear)
   ↓
4. Priority Queue
   ├─ EXIT signals: Highest priority
   ├─ SHIELD signals: Medium priority
   └─ SPEAR signals: Lower priority
   ↓
5. Trade Execution Engine
   ├─ RPC Mode Selection (Jito vs Standard)
   ├─ Transaction Building
   ├─ Jito Bundle Submission (if enabled)
   └─ Transaction Confirmation
   ↓
6. Position Tracking
   ├─ Database Update
   ├─ Price Cache Update
   └─ WebSocket Notification
```

### RPC Fallback Flow

```
Primary RPC (Helius + Jito)
   ↓
[Failure Count >= 3]
   ↓
Fallback RPC (QuickNode Standard)
   ├─ Spear Strategy Disabled
   ├─ Shield Strategy Continues
   └─ Auto-Recovery Every 5 Minutes
   ↓
[Primary RPC Health Check Passes]
   ↓
Recovery to Primary RPC
```

---

## State Machine

### Trade Status Lifecycle

```
PENDING
  ↓
QUEUED
  ↓
EXECUTING
  ├─→ ACTIVE (success)
  └─→ FAILED (error)
      ↓
      RETRY (if retries < max)
      ↓
      EXECUTING
      ↓
      [Max retries]
      ↓
      DEAD_LETTER

ACTIVE
  ↓
EXITING (exit signal received)
  ↓
CLOSED (exit confirmed)
```

### Position State Lifecycle

```
ACTIVE
  ↓
EXITING (exit signal)
  ↓
CLOSED (exit confirmed)
```

**State Transitions:**
- `PENDING → QUEUED`: Signal validated
- `QUEUED → EXECUTING`: Transaction building started
- `EXECUTING → ACTIVE`: Transaction confirmed on-chain
- `EXECUTING → FAILED`: Transaction rejected
- `FAILED → RETRY`: Retry count < max
- `RETRY → EXECUTING`: Retry attempt
- `ACTIVE → EXITING`: Exit signal received
- `EXITING → CLOSED`: Exit transaction confirmed

---

## Database Schema

### Core Tables

**trades**
- Primary record of all trading signals
- Tracks status, PnL, transaction signatures
- Indexed by status, wallet, token, created_at

**positions**
- Active positions being tracked
- Links to trades via trade_uuid
- Tracks unrealized PnL, current price

**wallets**
- Tracked wallets with WQS scores
- Managed by Scout service
- Status: ACTIVE, CANDIDATE, REJECTED

**reconciliation_log**
- On-chain vs DB state discrepancies
- Auto-resolution for minor differences
- Manual review for significant issues

**config_audit**
- All configuration changes
- Tracks who changed what and when
- Immutable audit trail

**dead_letter_queue**
- Failed operations requiring manual review
- Reasons: QUEUE_FULL, PARSE_ERROR, VALIDATION_FAILED, MAX_RETRIES

### Database Features

- **WAL Mode**: Write-Ahead Logging for concurrent reads
- **Busy Timeout**: 5 seconds for lock handling
- **Foreign Keys**: Enabled for referential integrity
- **Indexes**: Optimized for common queries

---

## RPC Architecture

### Primary RPC (Helius + Jito)

**Jito Mode:**
- Bundle submission for transaction prioritization
- Supports both Shield and Spear strategies
- Optimal latency and success rate

**Fallback Conditions:**
- 3 consecutive RPC failures
- Jito service unavailable
- Network connectivity issues

### Fallback RPC (QuickNode Standard)

**Standard Mode:**
- Direct transaction submission
- Spear strategy automatically disabled
- Shield strategy continues normally

**Recovery:**
- Automatic health check every 5 minutes
- Switches back when primary RPC recovers
- Logged to config_audit table

---

## Security Architecture

### Authentication Methods

1. **HMAC Webhook Authentication**
   - Signature: `HMAC-SHA256(timestamp + payload, SECRET)`
   - Timestamp validation: ±5 minutes
   - Replay protection via timestamp window

2. **Bearer Token Authentication**
   - API keys stored in `admin_wallets` table
   - Role-based access control (readonly, operator, admin)
   - JWT tokens for wallet-based auth

3. **Wallet Signature Authentication**
   - Solana wallet signature verification
   - Message: "Chimera authentication message"
   - Returns JWT for subsequent requests

### Secret Management

- **Webhook Secret**: Rotated every 30 days
- **RPC API Keys**: Rotated every 90 days
- **Trading Wallet Keypair**: Encrypted in vault
- **Grace Period**: 24 hours for secret rotation

---

## Circuit Breaker System

### Trip Conditions

1. **Max Loss (24h)**: Total losses exceed threshold
2. **Consecutive Losses**: Too many losses in a row
3. **Max Drawdown**: Portfolio drawdown exceeds limit

### States

- **CLOSED**: Normal operation, trading allowed
- **OPEN**: Circuit tripped, trading halted
- **HALF_OPEN**: Cooldown period, testing recovery

### Recovery

- Automatic cooldown period (configurable)
- Manual reset via API (admin only)
- Logged to config_audit table

---

## Reconciliation Process

### Daily Reconciliation

Runs automatically at 4 AM via cron (`ops/reconcile.sh`).

**Process:**
1. Query all ACTIVE and EXITING positions
2. Check on-chain transaction status
3. Compare DB state vs on-chain state
4. Log discrepancies to `reconciliation_log` table
5. Auto-resolve minor differences (within epsilon)
6. Alert on unresolved discrepancies

### Discrepancy Types

- **SIGNATURE_MISMATCH**: DB signature doesn't match on-chain
- **MISSING_TRANSACTION**: Position in DB but no on-chain TX
- **AMOUNT_MISMATCH**: Amount difference (auto-resolved if < 0.01%)
- **STATE_MISMATCH**: State difference (e.g., EXITING vs CLOSED)

### Auto-Resolution

- **Epsilon Tolerance**: 0.0001 SOL (0.01%)
- **Auto-resolve**: Amount differences within epsilon
- **Manual Review**: Significant discrepancies or missing transactions

---

## Performance Optimizations

### Queue Management

- **Priority Queue**: EXIT > SHIELD > SPEAR
- **Load Shedding**: Drop lower-priority signals when queue > 800
- **Rate Limiting**: 100 req/s webhook limit

### Database Optimizations

- **WAL Mode**: Concurrent reads during writes
- **Indexes**: Optimized for common query patterns
- **Connection Pooling**: Configurable pool size

### Caching

- **Price Cache**: In-memory token price cache
- **Token Metadata Cache**: Reduces RPC calls
- **TTL-based Expiration**: Automatic cache refresh

---

## Monitoring & Observability

### Metrics (Prometheus)

- **Trade Execution Latency**: p50, p95, p99
- **Queue Depth**: Current queue size
- **RPC Latency**: Primary and fallback RPC response times
- **Circuit Breaker State**: Open/Closed status
- **Error Rates**: By type and endpoint

### Logging

- **Structured Logging**: JSON format with tracing
- **Log Levels**: ERROR, WARN, INFO, DEBUG
- **Rotation**: Daily rotation with 7-day retention

### Alerts

- **Wallet Drain**: Balance drop > 5 SOL
- **Circuit Breaker Trip**: Trading halted
- **RPC Fallback**: Primary RPC unavailable
- **Reconciliation Discrepancies**: Unresolved issues

---

## Deployment Architecture

### Service Components

```
┌─────────────────┐
│  Load Balancer  │
└────────┬────────┘
         │
    ┌────┴────┐
    │         │
┌───▼───┐ ┌──▼────┐
│Operator│ │  Web │
│Service │ │  UI  │
└───┬───┘ └───────┘
    │
┌───▼──────────┐
│   SQLite     │
│  (WAL Mode)  │
└──────────────┘
```

### High Availability

- **Database Backups**: Daily automated backups
- **RPC Fallback**: Automatic failover
- **Circuit Breaker**: Risk protection
- **Health Checks**: Load balancer integration

---

## Troubleshooting Guide

### Common Issues

#### 1. Trades Not Executing

**Symptoms:**
- Signals accepted but no transactions
- Queue depth increasing

**Diagnosis:**
```bash
# Check circuit breaker status
curl http://localhost:8080/api/v1/health | jq '.circuit_breaker'

# Check queue depth
curl http://localhost:8080/api/v1/health | jq '.queue_depth'

# Check RPC status
curl http://localhost:8080/api/v1/config | jq '.rpc_status'

# Check logs for errors
journalctl -u chimera -n 100 | grep -i error
```

**Solutions:**
- Reset circuit breaker if tripped
- Check RPC connectivity
- Verify wallet has sufficient balance
- Check for rate limiting

#### 2. RPC Fallback Activated

**Symptoms:**
- Logs show "Switching to fallback RPC mode"
- Spear trades being rejected

**Diagnosis:**
```bash
# Check RPC mode
curl http://localhost:8080/api/v1/config | jq '.rpc_status.fallback_triggered'

# Test primary RPC
curl -X POST "${HELIUS_RPC_URL}" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}'
```

**Solutions:**
- Wait for automatic recovery (5 minutes)
- Check Helius status page
- Verify API key is valid
- Check network connectivity

#### 3. Database Locked

**Symptoms:**
- Logs show "database is locked" or "SQLITE_BUSY"
- Service may crash

**Diagnosis:**
```bash
# Check for stale processes
fuser /opt/chimera/data/chimera.db

# Check database integrity
sqlite3 /opt/chimera/data/chimera.db "PRAGMA integrity_check;"

# Check WAL mode
sqlite3 /opt/chimera/data/chimera.db "PRAGMA journal_mode;"
```

**Solutions:**
- WAL mode should allow concurrent access
- Check for long-running transactions
- Verify busy_timeout is set (5 seconds)
- Restart service if needed

#### 4. High Memory Usage

**Symptoms:**
- Service killed by OOM killer
- High memory consumption

**Diagnosis:**
```bash
# Check memory usage
ps aux | grep chimera_operator

# Check system memory
free -h

# Check OOM killer logs
dmesg | grep -i oom
```

**Solutions:**
- Increase systemd MemoryMax limit
- Enable load shedding (queue_depth > 800)
- Add swap space if needed
- Review price cache size limits

#### 5. Reconciliation Discrepancies

**Symptoms:**
- Unresolved discrepancies in reconciliation_log
- Positions not matching on-chain state

**Diagnosis:**
```bash
# Check unresolved discrepancies
sqlite3 /opt/chimera/data/chimera.db "
SELECT COUNT(*) FROM reconciliation_log
WHERE resolved_at IS NULL;"

# View recent discrepancies
sqlite3 /opt/chimera/data/chimera.db "
SELECT * FROM reconciliation_log
WHERE resolved_at IS NULL
ORDER BY created_at DESC
LIMIT 10;"
```

**Solutions:**
- Review reconciliation runbook
- Manually resolve significant discrepancies
- Check for transaction retries
- Verify on-chain transaction status

---

## Diagnostic Commands

### System Health
```bash
# Full health check
curl http://localhost:8080/api/v1/health | jq .

# Check specific component
curl http://localhost:8080/api/v1/health | jq '.circuit_breaker'
curl http://localhost:8080/api/v1/health | jq '.rpc'
curl http://localhost:8080/api/v1/health | jq '.database'
```

### Database Queries
```bash
# Active positions count
sqlite3 /opt/chimera/data/chimera.db "SELECT COUNT(*) FROM positions WHERE state = 'ACTIVE';"

# Recent trades
sqlite3 /opt/chimera/data/chimera.db "
SELECT trade_uuid, strategy, status, created_at
FROM trades
ORDER BY created_at DESC
LIMIT 10;"

# Queue depth (if stored)
sqlite3 /opt/chimera/data/chimera.db "SELECT COUNT(*) FROM trades WHERE status = 'QUEUED';"
```

### Log Analysis
```bash
# Recent errors
journalctl -u chimera -n 100 --no-pager | grep -i error

# RPC failures
journalctl -u chimera -n 100 --no-pager | grep -i "rpc\|fallback"

# Circuit breaker events
journalctl -u chimera -n 100 --no-pager | grep -i "circuit"
```

---

## Performance Tuning

### Database
- **WAL Mode**: Enabled for concurrent access
- **Busy Timeout**: 5 seconds (adjust if needed)
- **Connection Pool**: Default 10 connections
- **VACUUM**: Run periodically to reclaim space

### RPC
- **Rate Limit**: 40 req/s for Helius (adjust based on plan)
- **Timeout**: 2 seconds (adjust for network conditions)
- **Retry Logic**: 3 consecutive failures trigger fallback

### Queue
- **Max Depth**: 1000 signals
- **Load Shedding**: Starts at 800 depth
- **Priority**: EXIT > SHIELD > SPEAR

---

## Backup & Recovery

### Backup Strategy

- **Frequency**: Daily at 3 AM
- **Retention**: 7 days
- **Method**: SQLite VACUUM INTO
- **Verification**: SHA256 checksum

### Recovery Procedures

1. **Database Corruption**: Restore from backup
2. **Service Crash**: Automatic restart via systemd
3. **RPC Failure**: Automatic fallback
4. **Data Loss**: Restore from backup + reconciliation

See `ops/runbooks/` for detailed recovery procedures.

---

## Scaling Considerations

### Horizontal Scaling

Currently designed for single-instance deployment. For scaling:

1. **Database**: Move to PostgreSQL for multi-instance support
2. **Queue**: Use Redis/RabbitMQ for distributed queue
3. **State**: Externalize state management
4. **Load Balancing**: Multiple operator instances behind LB

### Vertical Scaling

- **Memory**: Increase for larger price cache
- **CPU**: More cores for parallel transaction building
- **Disk**: Faster SSD for database I/O

---

## Security Considerations

### Secret Management

- All secrets loaded from environment variables
- No secrets in code or config files
- Encrypted vault for trading wallet keypair
- Regular secret rotation

### Network Security

- HTTPS/TLS for all external communication
- Firewall rules for webhook endpoint
- IP allowlisting (optional)
- Rate limiting to prevent abuse

### Access Control

- Role-based access control (RBAC)
- API key rotation
- Wallet signature verification
- Audit logging for all changes

---

## Development Workflow

### Local Development

```bash
# Start operator
cd operator && cargo run

# Start web UI
cd web && npm run dev

# Run tests
cd operator && cargo test
cd scout && pytest
```

### Testing

- **Unit Tests**: Rust and Python
- **Integration Tests**: Full API workflow
- **Load Tests**: k6 scripts for performance
- **Chaos Tests**: Failure scenario testing
- **E2E Tests**: Playwright for UI

---

## Future Enhancements

### Planned Features

1. **Multi-Instance Support**: Distributed deployment
2. **Advanced Analytics**: ML-based wallet scoring
3. **Risk Management**: Dynamic position sizing
4. **Multi-Chain Support**: Extend beyond Solana

### Technical Debt

1. ✅ All TODO placeholders have been replaced with actual implementations (PDD v7.1 complete)
2. ✅ Comprehensive error recovery implemented (circuit breakers, fallback RPC, graceful degradation)
3. ✅ Monitoring and alerting enhanced (Prometheus metrics, Alertmanager, Grafana dashboards)
4. ✅ Documentation improved (PDD compliance, runbooks, API docs, test coverage)

### Future Enhancements (Optional)

1. Direct Jito Searcher integration (currently using Helius Sender API)
2. Raydium/Orca pool enumeration for enhanced liquidity detection
3. Property-based testing for WQS calculation
4. Performance benchmarks for high-frequency scenarios

---

## References

- **Product Design Document**: `docs/pdd.md`
- **API Documentation**: `docs/api.md`
- **Runbooks**: `ops/runbooks/`
- **Deployment Scripts**: `ops/deploy.sh`, `ops/rollback.sh`
