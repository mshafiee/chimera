# 🚀 10-Day Evaluation Startup - Quick Reference

## ✅ Configuration Status: COMPLETE

**API Key**: ✅ Configured (609cb910-17a5-4a76-9d1b-2ca9c42f759e)
**Network**: ✅ Validated (152ms response time)
**Historical Signals**: ✅ Generated (1,500 signals × 10 days)
**Infrastructure**: ✅ Ready (all services configured)

---

## 🎯 EXECUTION COMMAND

```bash
sudo ./ops/start-evaluation.sh evaluation
```

---

## 📊 WHAT TO EXPECT DURING STARTUP (3-5 minutes)

### Phase 1: Service Initialization (1-2 minutes)
```
✅ Docker Compose configuration validation
✅ Volume creation and permissions setup
✅ Network creation for evaluation services
✅ Environment file loading
```

### Phase 2: Service Startup (1-2 minutes)
```
✅ postgres-eval    - PostgreSQL database
✅ redis-eval       - Redis cache
✅ vector          - Log aggregation
✅ prometheus-eval  - Metrics collection
✅ grafana-eval     - Dashboards
✅ scout            - Python intelligence
✅ operator         - Rust trading engine
```

### Phase 3: Database Initialization (30 seconds)
```
✅ Schema creation (8 tables)
✅ Indexes and triggers
✅ Initial data snapshots
✅ Health check validation
```

### Phase 4: Signal Processing Start (1 minute)
```
✅ Historical signal replay begins
✅ 10x speed replay (150 signals/day compressed)
✅ Real-time webhook processing
✅ Trade execution simulation
```

---

## 🔍 STARTUP VALIDATION CHECKLIST

After running the startup command, verify these items:

### 1. Docker Services Status
```bash
docker-compose ps
```
**Expected**: All 7 services showing "Up" status

### 2. Operator Health Check
```bash
curl http://localhost:8080/api/v1/health
```
**Expected**: `{"status":"healthy","timestamp":"..."}`

### 3. Scout Health Check
```bash
curl http://localhost:8081/health
```
**Expected**: `{"status":"ok","services":{...}}`

### 4. Prometheus Metrics
```bash
curl http://localhost:9091/-/healthy
```
**Expected**: `Prometheus is Healthy.`

### 5. Grafana Dashboard
Open browser: `http://localhost:3003`
**Expected**: Grafana login page (admin / your configured password)

### 6. Database Validation
```bash
sqlite3 evaluation/evaluation.db ".tables"
```
**Expected**: 8 tables listed

### 7. Signal Replay Progress
```bash
tail -f evaluation/replay-results.log
```
**Expected**: Signal replay log with timestamps

---

## 🎛️ REAL-TIME MONITORING

### Launch Monitoring Dashboard
```bash
./ops/monitor-evaluation.sh
```

**This displays**:
- Service health status
- Resource usage (CPU/Memory/Disk)
- Active anomalies
- Signal replay progress
- Data collection status

### Check Anomaly Detection
```bash
python3 ops/detect-anomalies.py
```

### View Live Logs
```bash
# Operator logs
docker-compose logs --tail=50 operator

# All services
docker-compose logs --tail=20
```

---

## 📈 FIRST HOUR EXPECTATIONS

### Minutes 0-15: Service Stabilization
- Services starting and health checks
- Database initialization
- First signal replay attempts

### Minutes 15-30: Signal Processing
- Historical signals replaying at 10x speed
- First trades being processed
- Metrics collection beginning

### Minutes 30-60: Normal Operation
- Steady signal processing
- First hourly snapshot collection
- Anomaly detection active
- Monitoring dashboards populated

---

## ⚠️ COMMON STARTUP ISSUES

### Issue: Service fails to start
**Solution**: Check logs `docker-compose logs <service>`

### Issue: Port already in use
**Solution**: Stop conflicting services `docker-compose down`

### Issue: Database connection error
**Solution**: Verify postgres-eval container is healthy

### Issue: Signal replay not starting
**Solution**: Check webhook secret matches in env files

---

## 🎯 DAY 1 TARGETS

**First 24 Hours**:
- ✅ Process ~150 historical signals (Day 1 signals)
- ✅ Collect 24 hourly snapshots
- ✅ Generate first daily report
- ✅ Monitor for anomalies
- ✅ Verify system stability

**Success Criteria**:
- Operator health: 95%+ uptime
- Signal processing: 90%+ success rate
- Data collection: 95%+ completeness
- Resource usage: <70% CPU, <70% memory

---

## 📞 SUPPORT & TROUBLESHOOTING

**For issues during startup**:
1. Check service logs: `docker-compose logs --tail=50`
2. Run diagnostics: `./ops/preflight-validation.sh`
3. Monitor resources: `docker stats --no-stream`
4. Check documentation: `ops/runbooks/evaluation-quick-reference.md`

---

## 🚀 READY TO BEGIN!

Execute the startup command now:

```bash
sudo ./ops/start-evaluation.sh evaluation
```

Then in a separate terminal, start monitoring:

```bash
./ops/monitor-evaluation.sh
```

**Evaluation duration**: 10 days (automated data collection)
**Historical replay**: Days 1-5 (10x speed)
**Real-time recording**: Days 6-10 (live signals)
**Final analysis**: Day 11+ (comprehensive reporting)

Good luck with your evaluation! 🎯