# Chimera 10-Day Evaluation Playbook

**Purpose**: Comprehensive operational guide for executing and managing 10-day paper trading evaluations with systematic data collection and analysis.

**Scope**: End-to-end evaluation lifecycle including preparation, execution, monitoring, incident response, and post-evaluation analysis.

**Version**: 1.0.0  
**Last Updated**: 2026-06-28

---

## Table of Contents

1. [Pre-Evaluation Preparation](#pre-evaluation-preparation)
2. [Day-by-Day Operations](#day-by-day-operations)
3. [Monitoring and Response Procedures](#monitoring-and-response-procedures)
4. [Incident Response](#incident-response)
5. [Post-Evaluation Analysis](#post-evaluation-analysis)
6. [Emergency Procedures](#emergency-procedures)
7. [Troubleshooting Guide](#troubleshooting-guide)

---

## Pre-Evaluation Preparation

### Phase 1: Environment Setup (Day -2 to Day -1)

#### 1.1 System Requirements Validation

**Hardware Requirements:**
- **CPU**: 4+ cores recommended for evaluation services
- **Memory**: 16GB+ RAM (8GB for base services + 8GB for evaluation stack)
- **Storage**: 100GB+ free space (50GB for logs + 50GB for database snapshots)
- **Network**: Stable internet connection with <50ms latency to Helius RPC

**Prerequisites Check:**
```bash
# Run system validation
./ops/preflight-check.sh

# Verify Docker installation
docker --version  # Should be 20.10+
docker-compose --version  # Should be 2.0+

# Check disk space
df -h  # Minimum 100GB available

# Verify network latency
ping -c 5 mainnet.helius-rpc.com  # Should be <50ms
```

#### 1.2 Configuration Setup

**Environment Files Configuration:**
```bash
# Create evaluation environment files
cp docker/env.evaluation.local.example docker/env.evaluation.local

# Edit with your credentials
nano docker/env.evaluation.local
```

**Required Configuration:**
- `HELIUS_API_KEY`: Valid Helius API key for mainnet evaluation
- `CHIMERA_SECURITY__WEBHOOK_SECRET`: Generate with `openssl rand -hex 32`
- `POSTGRES_EVAL_PASSWORD`: Secure password for evaluation database
- `GRAFANA_ADMIN_PASSWORD`: Admin password for evaluation dashboards
- `TELEGRAM_BOT_TOKEN` + `TELEGRAM_CHAT_ID`: For alerts (recommended)
- `DISCORD_WEBHOOK_URL`: Alternative alert channel

**Generate Webhook Secret:**
```bash
openssl rand -hex 32
```

#### 1.3 Infrastructure Preparation

**Create Directory Structure:**
```bash
# Create evaluation directories
mkdir -p evaluation/{signals,profiles,network-captures,reports,backup}
mkdir -p evaluation/logs/evaluation
mkdir -p evaluation/prometheus
mkdir -p evaluation/grafana
mkdir -p evaluation/postgres-backup

# Set permissions
chmod 755 evaluation
chmod 700 docker/env.evaluation.local
```

**Download Historical Signals (if available):**
```bash
# Place historical signals for Days 1-5 replay
# Format: JSONL with timestamp, wallet_address, token_address, action, amount_sol
cp /path/to/historical-signals.jsonl evaluation/signals/
```

#### 1.4 Pre-Evaluation Testing

**Test Infrastructure Components:**
```bash
# Test Docker Compose configuration
docker-compose -f docker-compose.yml -f docker-compose.evaluation.yml --profile evaluation config

# Test individual services
docker-compose --profile evaluation up -d operator
docker-compose --profile evaluation up -d scout
curl http://localhost:8080/api/v1/health  # Should return 200
curl http://localhost:8081/health  # Should return 200

# Test evaluation services
docker-compose --profile evaluation up -d fluentd
docker-compose --profile evaluation up -d prometheus-eval
docker-compose --profile evaluation up -d postgres-eval
```

**Test Data Collection:**
```bash
# Run single data collection test
DAY_NUM=0 HOUR_NUM=0 ./ops/collect-evaluation-data.sh

# Verify evaluation database creation
sqlite3 evaluation/evaluation.db ".tables"
# Should show: evaluation_snapshots, trade_execution_details, etc.
```

### Phase 2: Pre-Evaluation Checklist (Day -1)

**Complete this checklist before starting evaluation:**

- [ ] System requirements validated (CPU, RAM, storage, network)
- [ ] All environment variables configured in `docker/env.evaluation.local`
- [ ] Webhook secret generated and configured
- [ ] Helius API key validated and working
- [ ] Historical signals file prepared (if using replay mode)
- [ ] Telegram/Discord notifications configured and tested
- [ ] Docker Compose services tested individually
- [ ] Data collection script tested successfully
- [ ] Evaluation database schema created and validated
- [ ] Monitoring dashboards accessible (Grafana, Prometheus)
- [ ] Sufficient disk space available (100GB+)
- [ ] Network latency to RPC <50ms verified
- [ ] Backup procedures tested
- [ ] Emergency procedures reviewed
- [ ] Team notification channels configured

---

## Day-by-Day Operations

### Day 0: Evaluation Startup

#### 0.1 Initial Startup

**Start Evaluation Services:**
```bash
# Launch evaluation stack
sudo ./ops/start-evaluation.sh evaluation

# Monitor startup process
tail -f /var/log/syslog | grep chimera
```

**Verify All Services:**
```bash
# Check all containers running
docker-compose ps

# Verify service health
curl http://localhost:8080/api/v1/health
curl http://localhost:8081/health
curl http://localhost:9091/-/healthy
```

#### 0.2 Initial Validation

**Run First Hour Validation:**
```bash
# Manually trigger first data collection
DAY_NUM=1 HOUR_NUM=0 ./ops/collect-evaluation-data.sh

# Verify data in evaluation database
sqlite3 evaluation/evaluation.db "SELECT COUNT(*) FROM evaluation_snapshots;"
# Should return 1

# Check Fluentd log collection
ls -la evaluation/logs/evaluation/
```

**Start Monitoring:**
```bash
# Launch real-time monitoring
./ops/monitor-evaluation.sh

# Check monitoring output for any immediate issues
```

#### 0.3 Signal Processing Setup

**Days 1-5: Historical Signal Replay**
```bash
# Start signal replay (background)
nohup python3 ops/signal-replayer.py \
    --signal-file evaluation/signals/historical-signals.jsonl \
    --webhook-url http://localhost:8080/api/v1/webhook \
    --webhook-secret ${CHIMERA_SECURITY__WEBHOOK_SECRET} \
    --replay-speed 10.0 \
    > evaluation/signal-replay.log 2>&1 &

# Monitor replay progress
tail -f evaluation/signal-replay.log
```

**Days 6-10: Real-Time Signal Recording**
```bash
# Start signal collector (Day 6)
python3 ops/signal-collector.py \
    --output-dir evaluation/signals/realtime \
    --duration-days 5 \
    --intercept-port 8090
```

### Day 1-5: Historical Replay Phase

#### Daily Routine (Morning)

**1. System Health Check:**
```bash
# Check overnight status
curl http://localhost:8080/api/v1/health | jq '.status'
curl http://localhost:9091/api/v1/query?query=up

# Review overnight anomalies
sqlite3 evaluation/evaluation.db \
    "SELECT COUNT(*) FROM evaluation_anomalies WHERE resolved = 0;"
```

**2. Data Collection Verification:**
```bash
# Check last collection time
find evaluation/day-1 -name "collection-summary-*.json" -mtime -1

# Verify hourly snapshots
sqlite3 evaluation/evaluation.db \
    "SELECT COUNT(*) FROM evaluation_snapshots WHERE day_number = 1;"
# Should equal number of hours since start
```

**3. Generate Daily Report:**
```bash
# Generate and review daily report
./ops/generate-daily-report.sh 1

# Review report HTML
open evaluation/day-1/reports/daily-report-day-1.html
```

#### Daily Routine (Evening)

**1. Performance Review:**
```bash
# Check daily performance metrics
sqlite3 evaluation/evaluation.db \
    "SELECT AVG(avg_trade_latency_ms), SUM(total_trades_today) 
     FROM evaluation_snapshots WHERE day_number = 1;"
```

**2. Anomaly Review:**
```bash
# Review today's anomalies
sqlite3 evaluation/evaluation.db \
    "SELECT severity, COUNT(*) FROM evaluation_anomalies 
     WHERE day_number = 1 GROUP BY severity;"

# Investigate critical anomalies
sqlite3 evaluation/evaluation.db \
    "SELECT * FROM evaluation_anomalies 
     WHERE day_number = 1 AND severity = 'CRITICAL';"
```

**3. Backup Verification:**
```bash
# Verify daily backups exist
ls -la evaluation/postgres-backup/
ls -la evaluation/day-1/database/
```

#### Signal Replay Monitoring

**Monitor Replay Progress:**
```bash
# Check replay log
tail -f evaluation/signal-replay.log

# Verify replay statistics
grep "Signal Replay Complete" evaluation/signal-replay.log
```

### Day 6-10: Real-Time Signal Phase

#### Day 6: Transition Day

**1. Stop Historical Replay:**
```bash
# Stop signal replayer
pkill -f signal-replayer.py

# Verify replay completion
sqlite3 evaluation/evaluation.db \
    "SELECT COUNT(*) FROM signal_replay_log WHERE replay_status = 'PENDING';"
```

**2. Start Real-Time Recording:**
```bash
# Launch signal collector
python3 ops/signal-collector.py \
    --output-dir evaluation/signals/realtime \
    --duration-days 5 \
    --intercept-port 8090 &
    
# Save PID for monitoring
echo $! > evaluation/signal-collector.pid
```

**3. Update Webhook Configuration:**
```bash
# If using external signal providers, update webhook URLs
# Edit docker/env.evaluation.local
# SIGNAL_PROVIDER_URL=https://your-signal-provider.com/webhook

# Reload services
docker-compose restart operator
```

#### Daily Routine (Days 6-10)

**Same as Days 1-5, plus:**

**Signal Collection Monitoring:**
```bash
# Check signal collector status
ps aux | grep signal-collector.py

# Verify signals being recorded
ls -la evaluation/signals/realtime/
tail -f evaluation/signals/realtime/recording-summary.json
```

**Real-Time Performance Comparison:**
```bash
# Compare real-time vs replay performance
sqlite3 evaluation/evaluation.db \
    "SELECT day_number, AVG(avg_trade_latency_ms), SUM(total_trades_today) 
     FROM evaluation_snapshots WHERE day_number >= 6 GROUP BY day_number;"
```

### Day 10: Evaluation Conclusion

#### 10.1 Final Data Collection

**Final Comprehensive Collection:**
```bash
# Run final data collection
DAY_NUM=10 HOUR_NUM=23 ./ops/collect-evaluation-data.sh

# Generate final daily report
./ops/generate-daily-report.sh 10
```

#### 10.2 Service Shutdown

**Orderly Service Shutdown:**
```bash
# Stop signal collection
pkill -f signal-collector.py

# Stop monitoring
pkill -f monitor-evaluation.sh

# Stop anomaly detection
pkill -f detect-anomalies.py

# Stop data collection cron
crontab -l | grep -v "collect-evaluation-data.sh" | crontab -

# Stop evaluation services
docker-compose --profile evaluation down

# Preserve evaluation data
tar -czf evaluation-backup-$(date +%Y%m%d).tar.gz evaluation/
```

---

## Monitoring and Response Procedures

### Continuous Monitoring

#### Real-Time Monitoring Dashboard

**Launch Comprehensive Monitoring:**
```bash
# Start monitoring in dedicated terminal
./ops/monitor-evaluation.sh

# Monitor key indicators:
# - Service health status
# - CPU/Memory usage
# - Queue depth
# - Active anomalies
# - Disk space
# - Data collection status
```

#### Grafana Dashboard Monitoring

**Key Dashboards to Monitor:**
1. **System Overview** (http://localhost:3003/d/system-overview)
   - Overall system health
   - Service status
   - Resource usage trends

2. **Performance Dashboard** (http://localhost:3003/d/performance)
   - Trade latency metrics
   - RPC performance
   - Queue depth trends
   - Success rates

3. **Cost Analysis Dashboard** (http://localhost:3003/d/costs)
   - Trading costs breakdown
   - Cost per trade trends
   - Jito tip analysis

4. **Risk Dashboard** (http://localhost:3003/d/risk)
   - Circuit breaker status
   - Drawdown metrics
   - Portfolio exposure

#### Prometheus Query Examples

**Critical Queries to Monitor:**
```bash
# Check error rate
rate(chimera_errors_total[5m]) > 0.1

# Monitor trade latency
histogram_quantile(0.95, chimera_trade_latency_duration_seconds)

# Queue depth alert
chimera_queue_depth > 800

# Circuit breaker status
chimera_circuit_breaker_state == 1

# RPC failure rate
rate(chimera_rpc_errors_total[5m]) / rate(chimera_rpc_calls_total[5m]) > 0.05
```

### Automated Alert Response

#### Alert Triage Procedure

**1. Alert Reception:**
- Telegram/Discord notification received
- Check monitoring dashboard
- Identify affected component

**2. Initial Assessment:**
```bash
# Check service health
curl http://localhost:8080/api/v1/health

# Check recent logs
tail -100 /var/log/chimera/operator.log | grep ERROR

# Check database status
sqlite3 evaluation/evaluation.db \
    "SELECT * FROM evaluation_anomalies ORDER BY anomaly_time DESC LIMIT 10;"
```

**3. Severity Determination:**
- **CRITICAL**: Immediate response required (<15 min)
- **WARNING**: Monitor and investigate (<1 hour)
- **INFO**: Log and review (<24 hours)

#### Standard Response Procedures

**For High CPU/Memory Alerts:**
```bash
# Check resource usage
docker stats chimera-operator chimera-scout

# Check process status
ps aux | grep chimera

# Restart if necessary
docker-compose restart operator

# Monitor recovery
./ops/monitor-evaluation.sh
```

**For Trade Latency Alerts:**
```bash
# Check RPC provider status
curl -w "@curl-format.txt" http://localhost:8080/api/v1/health

# Test RPC connectivity
time curl https://mainnet.helius-rpc.com/

# Check queue depth
curl http://localhost:8080/metrics | grep chimera_queue_depth

# Consider circuit breaker if latency >2000ms
```

**For Queue Depth Alerts:**
```bash
# Check current queue
curl http://localhost:8080/metrics | grep chimera_queue_depth

# Monitor for queue reduction
watch -n 5 'curl -s http://localhost:8080/metrics | grep chimera_queue_depth'

# Consider disabling Spear strategy if critical
```

---

## Incident Response

### Critical Incidents

#### Circuit Breaker Trips

**Immediate Response:**
```bash
# Check circuit breaker status
curl http://localhost:8080/api/v1/config/circuit-breaker

# Review recent trades
sqlite3 data/chimera.db "SELECT * FROM trades ORDER BY created_at DESC LIMIT 10;"

# Check system logs
tail -100 /var/log/chimera/operator.log | grep -i circuit

# Document incident
# Record: time, trigger reason, system state, actions taken
```

**Investigation Steps:**
1. Identify trigger condition (loss threshold, consecutive losses, drawdown)
2. Review recent trade performance
3. Check market conditions
4. Verify system stability
5. Document findings

**Recovery Procedure:**
```bash
# Only reset after investigation and fixes
curl -X POST http://localhost:8080/api/v1/config/circuit-breaker/reset

# Monitor post-reset performance
./ops/monitor-evaluation.sh
```

#### Service Failures

**Operator Service Down:**
```bash
# Check container status
docker-compose ps operator

# Review logs
docker-compose logs --tail=100 operator

# Restart service
docker-compose restart operator

# Verify recovery
curl http://localhost:8080/api/v1/health
```

**Database Issues:**
```bash
# Check database locks
sqlite3 evaluation/evaluation.db "PRAGMA database_list;"

# Restart database service
docker-compose restart postgres-eval

# Verify connectivity
docker-compose exec postgres-eval pg_isready -U chimera
```

### Data Collection Failures

#### Missing Hourly Snapshots

**Diagnosis:**
```bash
# Check last successful collection
find evaluation/ -name "collection-summary-*.json" -mtime -2

# Verify cron job
crontab -l | grep collect-evaluation-data

# Check collection logs
tail -100 /var/log/chimera/collector.log
```

**Resolution:**
```bash
# Manually trigger collection
DAY_NUM=<current_day> HOUR_NUM=<current_hour> ./ops/collect-evaluation-data.sh

# Verify data stored
sqlite3 evaluation/evaluation.db \
    "SELECT * FROM evaluation_snapshots WHERE day_number = <current_day> AND hour_number = <current_hour>;"

# Restart cron if needed
sudo service cron restart
```

#### Fluentd Log Collection Issues

**Diagnosis:**
```bash
# Check Fluentd status
docker-compose ps fluentd

# Review Fluentd logs
docker-compose logs --tail=100 fluentd

# Check log output
ls -la evaluation/logs/evaluation/
```

**Resolution:**
```bash
# Restart Fluentd
docker-compose restart fluentd

# Verify log collection
tail -f evaluation/logs/evaluation/operator-*.log
```

---

## Post-Evaluation Analysis

### Data Validation

#### 11.1 Data Completeness Check

**Verify 10-Day Coverage:**
```bash
# Check snapshot coverage
sqlite3 evaluation/evaluation.db \
    "SELECT day_number, COUNT(*) FROM evaluation_snapshots GROUP BY day_number;"

# Expected: 10 days with ~24 snapshots each

# Check for gaps
sqlite3 evaluation/evaluation.db \
    "SELECT day_number, hour_number FROM evaluation_snapshots 
     ORDER BY day_number, hour_number;" | \
    awk -F'|' '{if (NR>1) {if ($1!=prev_day || $2!=prev_hour+1) print "Gap at day "$1" hour "$2;} prev_day=$1; prev_hour=$2;}'
```

#### 11.2 Data Quality Validation

**Validate Key Metrics:**
```bash
# Check trade counts
sqlite3 evaluation/evaluation.db \
    "SELECT SUM(total_trades_today), SUM(successful_trades_today) FROM evaluation_snapshots;"

# Verify cost data
sqlite3 evaluation/evaluation.db \
    "SELECT AVG(total_cost_sol) FROM trade_execution_details;"

# Check anomaly completeness
sqlite3 evaluation/evaluation.db \
    "SELECT COUNT(*) FROM evaluation_anomalies WHERE resolved = 0;"
```

### Final Report Generation

#### 12.1 Comprehensive Report

**Generate Final Evaluation Report:**
```bash
# Generate comprehensive HTML report
python3 ops/generate-evaluation-report.py \
    --evaluation-dir evaluation \
    --database evaluation/evaluation.db \
    --output evaluation/FINAL_EVALUATION_REPORT.html

# Review report
open evaluation/FINAL_EVALUATION_REPORT.html
```

#### 12.2 Additional Analysis

**Performance Deep-Dive:**
```bash
# Compare Day 1 vs Day 10 performance
sqlite3 evaluation/evaluation.db \
    "SELECT day_number, AVG(avg_trade_latency_ms), AVG(rpc_latency_avg_ms) 
     FROM evaluation_snapshots WHERE day_number IN (1,10) GROUP BY day_number;"

# Analyze hourly patterns
sqlite3 evaluation/evaluation.db \
    "SELECT hour_number, AVG(avg_trade_latency_ms) FROM evaluation_snapshots 
     GROUP BY hour_number ORDER BY hour_number;"
```

**Cost Efficiency Analysis:**
```bash
# Calculate cost per trade
sqlite3 evaluation/evaluation.db \
    "SELECT AVG(total_cost_sol), AVG(jito_tip_sol), AVG(dex_fee_sol) 
     FROM trade_execution_details;"

# Analyze cost by strategy
sqlite3 evaluation/evaluation.db \
    "SELECT strategy, AVG(total_cost_sol) FROM trade_execution_details 
     GROUP BY strategy;"
```

### Data Archival

#### 13.1 Evaluation Data Packaging

**Create Complete Archive:**
```bash
# Create timestamped archive
EVAL_DATE=$(date +%Y%m%d)
tar -czf chimera-evaluation-${EVAL_DATE}.tar.gz \
    evaluation/ \
    data/chimera.db \
    ops/logs/evaluation/

# Calculate checksum
sha256sum chimera-evaluation-${EVAL_DATE}.tar.gz > chimera-evaluation-${EVAL_DATE}.sha256

# Verify archive integrity
sha256sum -c chimera-evaluation-${EVAL_DATE}.sha256
```

#### 13.2 Investigation-Ready Data Organization

**Organize for Future Analysis:**
```bash
# Create investigation directory structure
mkdir -p investigation-ready/by-hour
mkdir -p investigation-ready/by-category
mkdir -p investigation-ready/incidents
mkdir -p investigation-ready/performance
mkdir -p investigation-ready/costs

# Organize data by category
cp evaluation/signals/*.jsonl investigation-ready/by-category/
cp evaluation/day-*/metrics/*.txt investigation-ready/performance/
cp evaluation/day-*/database/*.db investigation-ready/by-hour/
```

---

## Emergency Procedures

### Emergency Stopping

#### Immediate Evaluation Halt

**Stop All Evaluation Activities:**
```bash
# Emergency stop script
./ops/emergency-stop-evaluation.sh

# Or manual stop
pkill -9 signal-replayer.py
pkill -9 signal-collector.py
pkill -9 detect-anomalies.py
pkill -9 monitor-evaluation.sh

# Stop all Docker services
docker-compose --profile evaluation down

# Stop data collection
crontab -l | grep -v "collect-evaluation-data.sh" | crontab -
```

### Emergency Recovery

#### Service Recovery

**Restart Evaluation Services:**
```bash
# Restart core services
docker-compose up -d operator scout

# Restart evaluation services
docker-compose --profile evaluation up -d

# Restart monitoring
./ops/monitor-evaluation.sh &

# Resume data collection
echo "0 * * * * root $(pwd)/ops/collect-evaluation-data.sh" | crontab -
```

#### Data Recovery

**Database Recovery:**
```bash
# Restore from backup if needed
docker-compose exec postgres-eval \
    pg_restore -U chimera -d chimera_evaluation /backup/latest_backup.dump

# Verify recovery
sqlite3 evaluation/evaluation.db ".tables"
```

### Emergency Contact

**Critical Escalation Contacts:**
- **System Administrator**: [Contact details]
- **Development Team**: [Contact details]  
- **Infrastructure Team**: [Contact details]

**Emergency Communication Channels:**
- **Primary**: [Slack channel/Discord server]
- **Backup**: [Email distribution list]
- **Urgent**: [Phone tree]

---

## Troubleshooting Guide

### Common Issues and Solutions

#### Issue: Docker Services Won't Start

**Symptoms:**
```bash
docker-compose up -d
# Error: port is already allocated
# Error: network creation failed
```

**Solutions:**
```bash
# Check for port conflicts
netstat -tulpn | grep :8080

# Clean up existing containers
docker-compose down -v

# Remove orphaned containers
docker container prune

# Restart Docker daemon
sudo systemctl restart docker
```

#### Issue: Database Connection Errors

**Symptoms:**
```bash
sqlite3 evaluation/evaluation.db ".tables"
# Error: database is locked
# Error: no such table: evaluation_snapshots
```

**Solutions:**
```bash
# Check database file integrity
sqlite3 evaluation/evaluation.db "PRAGMA integrity_check;"

# Reinitialize schema if needed
cat database/evaluation_schema.sql | sqlite3 evaluation/evaluation.db

# Check file permissions
ls -la evaluation/evaluation.db
chmod 644 evaluation/evaluation.db
```

#### Issue: High Memory Usage

**Symptoms:**
```bash
docker stats
# chimera-operator: 85%+ memory usage
# chimera-scout: 75%+ memory usage
```

**Solutions:**
```bash
# Check for memory leaks
docker-compose logs operator | grep -i memory

# Restart services
docker-compose restart operator scout

# Clear evaluation database if needed
# (Archive first!)
mv evaluation/evaluation.db evaluation/evaluation.db.backup
sqlite3 evaluation/evaluation.db < database/evaluation_schema.sql
```

#### Issue: Missing Data in Reports

**Symptoms:**
- Daily report shows "N/A" for metrics
- Final report incomplete

**Solutions:**
```bash
# Verify data collection ran
ls -la evaluation/day-*/metrics/

# Manually collect missing data
DAY_NUM=<missing_day> HOUR_NUM=<missing_hour> ./ops/collect-evaluation-data.sh

# Regenerate report
./ops/generate-daily-report.sh <day_num>
```

#### Issue: Anomaly Detection Not Working

**Symptoms:**
```bash
ps aux | grep detect-anomalies.py
# No process found

# No anomalies in database despite issues
```

**Solutions:**
```bash
# Check anomaly detector logs
tail -100 evaluation/anomaly-detection.log

# Restart anomaly detection
python3 ops/detect-anomalies.py --interval 60 > evaluation/anomaly-detection.log 2>&1 &

# Verify Prometheus connectivity
curl http://localhost:9091/api/v1/query?query=up
```

### Diagnostic Commands

#### System Health Check

**Comprehensive Diagnostics:**
```bash
# Run full system diagnostics
./ops/diagnostic-check.sh

# Or individual checks
curl -s http://localhost:8080/api/v1/health | jq .
docker-compose ps
docker stats --no-stream
df -h
free -h
```

#### Data Integrity Check

**Verify Data Collection:**
```bash
# Check recent data
sqlite3 evaluation/evaluation.db \
    "SELECT MAX(snapshot_time) FROM evaluation_snapshots;"

# Check data completeness
sqlite3 evaluation/evaluation.db \
    "SELECT day_number, COUNT(*) FROM evaluation_snapshots GROUP BY day_number;"

# Verify logs
ls -la evaluation/logs/evaluation/
find evaluation/ -name "*.json" -mtime -1
```

---

## Appendix

### A. Quick Reference Commands

**Start Evaluation:**
```bash
sudo ./ops/start-evaluation.sh evaluation
```

**Start Monitoring:**
```bash
./ops/monitor-evaluation.sh
```

**Generate Daily Report:**
```bash
./ops/generate-daily-report.sh <day_num>
```

**Stop Evaluation:**
```bash
./ops/emergency-stop-evaluation.sh
```

**Check System Health:**
```bash
curl http://localhost:8080/api/v1/health
docker-compose ps
docker stats
```

### B. File Locations

**Configuration Files:**
- `docker-compose.evaluation.yml` - Evaluation services
- `docker/env.evaluation` - Base evaluation environment
- `docker/env.evaluation.local` - Local credentials (DO NOT COMMIT)

**Data Files:**
- `evaluation/evaluation.db` - Evaluation database
- `evaluation/day-*/` - Daily data organized by day
- `evaluation/logs/evaluation/` - Aggregated logs
- `evaluation/signals/` - Historical and real-time signals

**Script Files:**
- `ops/start-evaluation.sh` - Evaluation startup
- `ops/monitor-evaluation.sh` - Real-time monitoring
- `ops/collect-evaluation-data.sh` - Hourly data collection
- `ops/detect-anomalies.py` - Anomaly detection
- `ops/signal-replayer.py` - Historical signal replay
- `ops/generate-daily-report.sh` - Daily report generation
- `ops/generate-evaluation-report.py` - Final comprehensive report

### C. Port Reference

| Service | Port | Purpose |
|---------|------|---------|
| Operator | 8080 | Main trading API |
| Scout | 8081 | Wallet intelligence API |
| Prometheus Eval | 9091 | Evaluation metrics |
| Grafana Eval | 3003 | Evaluation dashboards |
| Fluentd | 24224 | Log aggregation |
| Signal Collector | 8090 | Real-time signal recording |

### D. Environment Variables

**Critical Variables:**
```bash
EVAL_DIR=/opt/chimera/evaluation
DAY_NUM=1-10
SIGNAL_MODE=replay|realtime
CHIMERA_SECURITY__WEBHOOK_SECRET=<64-char hex>
HELIUS_API_KEY=<your-key>
TELEGRAM_BOT_TOKEN=<your-token>
TELEGRAM_CHAT_ID=<your-chat-id>
DISCORD_WEBHOOK_URL=<your-webhook>
```

### E. Contact Information

**Evaluation Support:**
- **Documentation**: See CLAUDE.md and README.md
- **Issue Tracking**: GitHub Issues
- **Emergency Contacts**: See Emergency Procedures section

---

**End of Playbook**

For questions or issues during evaluation, refer to the relevant section above or consult the main Chimera documentation.