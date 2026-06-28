# 🚀 10-Day Evaluation Startup - Quick Guide

## ✅ STATUS: READY FOR LAUNCH

**All systems are go!** Execute the command below to begin your 10-day paper trading evaluation.

---

## 🎯 EXECUTION COMMAND

```bash
sudo ./ops/start-evaluation.sh evaluation
```

**Expected startup time**: 3-5 minutes

---

## 📊 WHAT HAPPENS AUTOMATICALLY

### Phase 1: Service Startup (1-2 minutes)
- ✅ PostgreSQL evaluation database
- ✅ Redis cache service  
- ✅ Vector log aggregation
- ✅ Prometheus metrics collection
- ✅ Grafana dashboards
- ✅ Scout Python intelligence
- ✅ Operator Rust trading engine

### Phase 2: Database Initialization (30 seconds)
- ✅ 8 evaluation tables created
- ✅ Indexes and triggers setup
- ✅ First data snapshot initialized
- ✅ Health checks validated

### Phase 3: Signal Processing (1 minute)
- ✅ Historical signal replay begins (1,500 signals)
- ✅ 10x speed replay (5 days compressed into ~12 hours)
- ✅ Realistic trading patterns simulated
- ✅ Performance metrics collection starts

### Phase 4: Monitoring Activation (30 seconds)
- ✅ Real-time monitoring dashboard
- ✅ Automated hourly data collection
- ✅ Anomaly detection system
- ✅ Alert notifications ready

---

## 🔍 STARTUP VALIDATION

After running the startup command, verify these items:

```bash
# 1. Check all services running
docker-compose ps
# Expected: 7 services showing "Up"

# 2. Test operator health
curl http://localhost:8080/api/v1/health
# Expected: {"status":"healthy",...}

# 3. Check database
sqlite3 evaluation/evaluation.db ".tables"
# Expected: 8 tables listed

# 4. Monitor signal replay
tail -f evaluation/replay-results.log
# Expected: Signal timestamps and processing
```

---

## 🎛️ REAL-TIME MONITORING

**Open a separate terminal** and run:

```bash
./ops/monitor-evaluation.sh
```

This displays:
- Service health status (color-coded)
- Resource usage (CPU/Memory/Disk)
- Signal replay progress
- Active anomalies count
- Data collection status

---

## 📈 EXPECTED TIMELINE

**Day 0** (Today): Startup and stabilization
- Services start and health checks
- Signal replay begins
- First data snapshots collected

**Days 1-5**: Historical replay phase
- ~150 signals processed per day
- Realistic trading simulation
- Performance metrics collected

**Days 6-10**: Real-time recording phase
- Signal collector starts
- Live signal capture
- Performance comparison

**Days 11+**: Analysis phase
- Comprehensive report generation
- Performance analysis
- Recommendations

---

## ⚠️ COMMON STARTUP QUESTIONS

**Q: How do I know it's working?**
A: Check `docker-compose ps` - all 7 services should show "Up" status

**Q: What if something goes wrong?**
A: Check service logs: `docker-compose logs <service-name>`

**Q: Can I monitor progress?**
A: Yes! Run `./ops/monitor-evaluation.sh` in a separate terminal

**Q: How long does the startup take?**
A: 3-5 minutes for all services to be healthy

**Q: What happens after startup?**
A: Signal replay begins automatically, data collection starts

---

## 🆘 EMERGENCY STOP

If needed, stop the evaluation:

```bash
./ops/emergency-stop-evaluation.sh
```

---

## 📞 SUPPORT

**Documentation**: `ops/runbooks/evaluation-quick-reference.md`  
**Comprehensive Guide**: `ops/runbooks/10-day-evaluation-playbook.md`

---

## 🎯 READY TO BEGIN!

Execute the startup command now:

```bash
sudo ./ops/start-evaluation.sh evaluation
```

**Your 10-day paper trading evaluation begins immediately!**

Good luck! 🚀