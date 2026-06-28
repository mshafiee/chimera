# Chimera 10-Day Evaluation Quick Reference

**Purpose**: At-a-glance guide for the 10-day paper trading evaluation process.

**For detailed procedures**: See [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md)

---

## 🚀 Quick Start (5 Minutes)

```bash
# 1. Setup environment (one-time)
cp docker/env.evaluation.local.example docker/env.evaluation.local
# Edit docker/env.evaluation.local with your credentials

# 2. Start evaluation
sudo ./ops/start-evaluation.sh evaluation

# 3. Monitor progress
./ops/monitor-evaluation.sh

# 4. Generate final report
python3 ops/generate-evaluation-report.py
```

---

## 📅 Daily Operations (2 Minutes)

**Morning Check:**
```bash
# Check service health
curl http://localhost:8080/api/v1/health

# Generate daily report
./ops/generate-daily-report.sh <day_num>

# Review overnight anomalies
sqlite3 evaluation/evaluation.db "SELECT COUNT(*) FROM evaluation_anomalies WHERE resolved = 0;"
```

**Evening Check:**
```bash
# Verify data collection
find evaluation/ -name "collection-summary-*.json" -mtime -1

# Check disk space
df -h evaluation/
```

---

## 🔥 Emergency Response (30 Seconds)

**Critical Issue - Stop Everything:**
```bash
./ops/emergency-stop-evaluation.sh
```

**High CPU/Memory:**
```bash
docker-compose restart operator scout
```

**Service Down:**
```bash
docker-compose ps
docker-compose restart <service_name>
```

**Data Collection Stopped:**
```bash
DAY_NUM=<current> HOUR_NUM=<current> ./ops/collect-evaluation-data.sh
```

---

## 📊 Key Metrics Dashboard

| Metric | Healthy | Warning | Critical |
|--------|---------|---------|----------|
| Trade Success Rate | >95% | 90-95% | <90% |
| Avg Trade Latency | <100ms | 100-200ms | >200ms |
| Queue Depth | <500 | 500-800 | >800 |
| CPU Usage | <70% | 70-85% | >85% |
| Memory Usage | <70% | 70-85% | >85% |
| Active Anomalies | 0 | 1-5 | >5 |

---

## 🎯 Dashboard URLs

- **Operator Health**: http://localhost:8080/api/v1/health
- **Grafana Eval**: http://localhost:3003
- **Prometheus Eval**: http://localhost:9091
- **Scout Health**: http://localhost:8081/health

---

## 📁 File Locations

| Component | Location |
|-----------|----------|
| Evaluation Data | `evaluation/` |
| Daily Reports | `evaluation/day-*/reports/` |
| Database | `evaluation/evaluation.db` |
| Logs | `evaluation/logs/evaluation/` |
| Signals | `evaluation/signals/` |
| Final Report | `evaluation/FINAL_EVALUATION_REPORT.html` |

---

## 🔧 Common Commands

**Check Services:**
```bash
docker-compose ps
docker stats --no-stream
```

**View Logs:**
```bash
docker-compose logs --tail=50 operator
tail -f evaluation/logs/evaluation/operator-*.log
```

**Database Queries:**
```bash
# Recent snapshots
sqlite3 evaluation/evaluation.db "SELECT * FROM evaluation_snapshots ORDER BY snapshot_time DESC LIMIT 10;"

# Active anomalies
sqlite3 evaluation/evaluation.db "SELECT * FROM evaluation_anomalies WHERE resolved = 0;"

# Daily summary
sqlite3 evaluation/evaluation.db "SELECT day_number, SUM(total_trades_today) FROM evaluation_snapshots GROUP BY day_number;"
```

---

## ⚠️ Alert Response

**1. Receive Alert** (Telegram/Discord)

**2. Quick Diagnosis (30 seconds):**
```bash
curl http://localhost:8080/api/v1/health
docker-compose ps
tail -50 /var/log/chimera/operator.log | grep ERROR
```

**3. Standard Response:**
- **Warning**: Monitor and investigate within 1 hour
- **Critical**: Immediate response (<15 minutes)
- **Service Down**: Restart affected service
- **Data Missing**: Manual collection trigger

---

## 📱 Monitoring Setup

**Launch All Monitoring:**
```bash
# Terminal 1: Real-time monitoring
./ops/monitor-evaluation.sh

# Terminal 2: Log monitoring
tail -f evaluation/logs/evaluation/operator-*.log

# Terminal 3: Anomaly detection
python3 ops/detect-anomalies.py
```

---

## 🔄 Signal Processing

**Days 1-5 (Historical Replay):**
```bash
python3 ops/signal-replayer.py \
    --signal-file evaluation/signals/historical-signals.jsonl \
    --webhook-url http://localhost:8080/api/v1/webhook \
    --webhook-secret ${CHIMERA_SECURITY__WEBHOOK_SECRET} \
    --replay-speed 10.0
```

**Days 6-10 (Real-Time Recording):**
```bash
python3 ops/signal-collector.py \
    --output-dir evaluation/signals/realtime \
    --duration-days 5 \
    --intercept-port 8090
```

---

## 📊 Report Generation

**Daily Reports:**
```bash
./ops/generate-daily-report.sh 1
./ops/generate-daily-report.sh 2
# ... etc
./ops/generate-daily-report.sh 10
```

**Final Comprehensive Report:**
```bash
python3 ops/generate-evaluation-report.py \
    --evaluation-dir evaluation \
    --database evaluation/evaluation.db \
    --output evaluation/FINAL_EVALUATION_REPORT.html
```

---

## 🗄️ Data Archival

**Post-Evaluation Archive:**
```bash
# Create complete archive
tar -czf chimera-evaluation-$(date +%Y%m%d).tar.gz evaluation/

# Verify archive
tar -tzf chimera-evaluation-*.tar.gz | head -20

# Calculate checksum
sha256sum chimera-evaluation-*.tar.gz
```

---

## 🆘 Emergency Contacts

| Issue Type | Contact | Response Time |
|------------|---------|---------------|
| System Failure | Infrastructure Team | 15 minutes |
| Data Issues | Development Team | 1 hour |
| Performance | Operations Team | 2 hours |
| Documentation | Project Lead | 24 hours |

---

## ✅ Pre-Flight Checklist

**Before Starting Evaluation:**
- [ ] Environment variables configured
- [ ] Webhook secret generated
- [ ] Helius API key validated
- [ ] Historical signals prepared (if using replay)
- [ ] Telegram/Discord notifications tested
- [ ] Sufficient disk space (100GB+)
- [ ] Network latency <50ms verified
- [ ] Backup procedures tested

**Daily Verification:**
- [ ] Services healthy
- [ ] Data collection running
- [ ] Monitoring active
- [ ] Disk space adequate
- [ ] No critical anomalies

---

## 🔍 Diagnostic Commands

**Full System Check:**
```bash
# Service status
docker-compose ps

# Resource usage
docker stats --no-stream

# Disk space
df -h

# Memory usage
free -h

# Network latency
ping -c 5 mainnet.helius-rpc.com

# Database integrity
sqlite3 evaluation/evaluation.db "PRAGMA integrity_check;"
```

---

## 📚 Documentation Links

- **Full Playbook**: [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md)
- **Main Documentation**: [../../docs/](../../docs/)
- **API Reference**: [../../docs/core/api.md](../../docs/core/api.md)
- **Troubleshooting**: [../../README.md](../../README.md)

---

## 🎓 Key Concepts

**Evaluation Phases:**
- **Days 0-1**: Infrastructure setup and validation
- **Days 1-5**: Historical signal replay (controlled testing)
- **Days 6-10**: Real-time signal recording (live validation)
- **Day 11+**: Analysis and reporting

**Data Collection:**
- **Hourly**: Automated system snapshots
- **Daily**: HTML performance reports
- **Final**: Comprehensive evaluation analysis

**Monitoring Levels:**
- **Real-time**: Service health, resource usage
- **Hourly**: Performance metrics, anomalies
- **Daily**: Trends, cost analysis, risk assessment

---

## ⏱️ Time Investments

| Activity | Time Required | Frequency |
|----------|---------------|-----------|
| Initial Setup | 2 hours | One-time |
| Daily Morning Check | 5 minutes | Daily |
| Daily Evening Check | 5 minutes | Daily |
| Critical Incident Response | 15-30 minutes | As needed |
| Final Report Generation | 30 minutes | End of evaluation |
| Data Archival | 1 hour | End of evaluation |

---

**For detailed procedures**, see the [comprehensive playbook](./10-day-evaluation-playbook.md).

**Questions?** Refer to main [README.md](../../README.md) or [CLAUDE.md](../../CLAUDE.md).