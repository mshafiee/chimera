# Chimera 10-Day Evaluation Documentation Index

**Version**: 1.0.0  
**Last Updated**: 2026-06-28  
**Status**: Production Ready  

---

## 📚 Documentation Overview

This documentation package provides complete operational guidance for executing 10-day paper trading evaluations of the Chimera trading platform. The evaluation infrastructure is designed to systematically collect, analyze, and report on all aspects of system performance in a controlled paper trading environment.

### Documentation Structure

```
ops/runbooks/
├── evaluation-guide-index.md              # This file - Master index
├── 10-day-evaluation-playbook.md         # Comprehensive operational guide
├── evaluation-quick-reference.md          # At-a-glance reference guide
├── evaluation-checklist.md                # Day-by-day progress tracking
└── evaluation-summary-template.md         # Post-evaluation reporting template
```

---

## 🎯 Quick Start Guide

### For First-Time Users

**1. Read This First** (15 minutes):
- Start with [evaluation-quick-reference.md](./evaluation-quick-reference.md)
- Understand the evaluation phases and daily routine

**2. Detailed Preparation** (1 hour):
- Review [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md)
- Complete pre-evaluation checklist
- Set up environment and infrastructure

**3. Begin Evaluation** (30 minutes):
- Use [evaluation-checklist.md](./evaluation-checklist.md) for tracking
- Start evaluation startup procedure
- Begin daily monitoring routine

**4. Post-Evaluation** (2 hours):
- Complete [evaluation-summary-template.md](./evaluation-summary-template.md)
- Generate comprehensive reports
- Document findings and recommendations

### For Experienced Operators

**Quick Reference** (5 minutes):
- [evaluation-quick-reference.md](./evaluation-quick-reference.md)
- Emergency procedures and commands

**Detailed Procedures** (as needed):
- [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md)
- Specific sections for incident response, troubleshooting

**Documentation** (post-evaluation):
- [evaluation-summary-template.md](./evaluation-summary-template.md)

---

## 📖 Detailed Documentation Guide

### 1. Evaluation Guide Index (This File)

**Purpose**: Master navigation and overview  
**When to Use**: First time accessing evaluation documentation  
**Key Sections**: Quick start, documentation overview, file locations

### 2. 10-Day Evaluation Playbook

**File**: [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md)  
**Purpose**: Comprehensive operational procedures  
**Length**: ~50 pages  
**When to Use**: 
- Pre-evaluation planning and setup
- Daily operational reference
- Incident response guidance
- Troubleshooting and emergency procedures

**Key Sections**:
- Pre-Evaluation Preparation (System requirements, configuration, testing)
- Day-by-Day Operations (Daily routines, monitoring, verification)
- Monitoring and Response Procedures (Real-time monitoring, automated alerts)
- Incident Response (Critical incidents, service failures, data collection failures)
- Post-Evaluation Analysis (Data validation, report generation, archival)
- Emergency Procedures (Emergency stopping, recovery, contacts)
- Troubleshooting Guide (Common issues, diagnostic commands)

**Sample Content**:
```bash
# Start evaluation
sudo ./ops/start-evaluation.sh evaluation

# Daily monitoring
./ops/monitor-evaluation.sh

# Generate reports
./ops/generate-daily-report.sh 1
```

### 3. Evaluation Quick Reference

**File**: [evaluation-quick-reference.md](./evaluation-quick-reference.md)  
**Purpose**: At-a-glance operational guide  
**Length**: ~5 pages  
**When to Use**: 
- Daily operational procedures
- Emergency response
- Quick command lookup
- On-the-job reference

**Key Sections**:
- Quick Start (5-minute setup)
- Daily Operations (2-minute routine)
- Emergency Response (30-second procedures)
- Key Metrics Dashboard (Health thresholds)
- Dashboard URLs and ports
- Common commands and diagnostics

**Sample Content**:
```bash
# Emergency stop (30 seconds)
./ops/emergency-stop-evaluation.sh

# Daily health check (2 minutes)
curl http://localhost:8080/api/v1/health
./ops/generate-daily-report.sh <day_num>
```

### 4. Evaluation Checklist

**File**: [evaluation-checklist.md](./evaluation-checklist.md)  
**Purpose**: Progress tracking and verification  
**Length**: ~15 pages  
**When to Use**: 
- During evaluation execution
- Progress verification
- Daily task completion
- Final sign-off

**Key Sections**:
- Pre-Evaluation Preparation Checklist
- Day 0: Evaluation Startup
- Days 1-5: Historical Replay Phase
- Days 6-10: Real-Time Signal Phase
- Post-Evaluation Analysis
- Incident Tracking
- Daily Performance Log
- Final Sign-Off

**Sample Content**:
- [ ] Environment variables configured
- [ ] Docker services tested
- [ ] Daily report generated
- [ ] Data collection verified

### 5. Evaluation Summary Template

**File**: [evaluation-summary-template.md](./evaluation-summary-template.md)  
**Purpose**: Post-evaluation documentation and reporting  
**Length**: ~20 pages  
**When to Use**: 
- After evaluation completion
- Management reporting
- Performance analysis documentation
- Recommendations compilation

**Key Sections**:
- Executive Summary (Key metrics, findings, recommendations)
- Performance Analysis (Trading performance, system performance, trends)
- Cost Analysis (Cost breakdown, efficiency, optimization opportunities)
- Risk Analysis (Circuit breaker performance, drawdown analysis)
- System Health Assessment (Health score, error analysis)
- Anomaly Analysis (Anomaly statistics, detection performance)
- Performance Trends (Day-to-day analysis, signal mode comparison)
- Recommendations (Critical, high, medium, low priority)
- Lessons Learned (Technical and operational insights)
- Final Assessment (Overall grade, production readiness)

**Sample Content**:
- Overall Grade: A
- Total Trades: 15,234
- Success Rate: 97.3%
- Net PnL: 12.45 SOL
- Health Score: 92/100

---

## 🚀 Documentation Usage Scenarios

### Scenario 1: First-Time Evaluation Setup

**Step 1: Planning Phase (Day -2 to -1)**
1. Read [evaluation-quick-reference.md](./evaluation-quick-reference.md) - 15 minutes
2. Review [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) - 1 hour
3. Complete pre-evaluation checklist from [evaluation-checklist.md](./evaluation-checklist.md)
4. Set up environment and infrastructure

**Step 2: Execution Phase (Day 0-10)**
1. Use [evaluation-checklist.md](./evaluation-checklist.md) for daily tracking
2. Reference [evaluation-quick-reference.md](./evaluation-quick-reference.md) for daily operations
3. Consult [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) for detailed procedures
4. Monitor using provided dashboards and commands

**Step 3: Post-Evaluation Phase (Day 11+)**
1. Complete [evaluation-summary-template.md](./evaluation-summary-template.md)
2. Generate final reports
3. Document findings and recommendations

### Scenario 2: Daily Operations

**Morning Routine (5 minutes)**
1. Check [evaluation-quick-reference.md](./evaluation-quick-reference.md) → "Daily Operations"
2. Run health check commands
3. Generate daily report
4. Verify overnight anomalies

**Evening Routine (5 minutes)**
1. Review daily performance metrics
2. Check backup completion
3. Update checklist progress

### Scenario 3: Incident Response

**Emergency Response (30 seconds to 15 minutes)**
1. Use [evaluation-quick-reference.md](./evaluation-quick-reference.md) → "Emergency Response"
2. Execute emergency procedures
3. Consult [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) → "Incident Response" for detailed guidance
4. Document incident in checklist
5. Update evaluation summary if needed

### Scenario 4: Troubleshooting

**Issue Resolution (5-30 minutes)**
1. Check [evaluation-quick-reference.md](./evaluation-quick-reference.md) → "Common Commands"
2. Consult [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) → "Troubleshooting Guide"
3. Run diagnostic commands
4. Implement resolution
5. Document for future reference

---

## 📊 File and Command Reference

### Configuration Files

| File | Purpose | Location |
|------|---------|----------|
| Docker Compose Eval | Evaluation services definition | `docker-compose.evaluation.yml` |
| Environment Base | Base evaluation variables | `docker/env.evaluation` |
| Environment Local | Local credentials | `docker/env.evaluation.local` |
| Vector Config | Log aggregation configuration | `ops/vector/vector.toml` |
| Evaluation Schema | Database schema | `database/evaluation_schema.sql` |

### Key Script Files

| Script | Purpose | Usage |
|--------|---------|-------|
| `start-evaluation.sh` | Start evaluation stack | `./ops/start-evaluation.sh evaluation` |
| `monitor-evaluation.sh` | Real-time monitoring | `./ops/monitor-evaluation.sh` |
| `collect-evaluation-data.sh` | Hourly data collection | Automated via cron |
| `detect-anomalies.py` | Anomaly detection | `python3 ops/detect-anomalies.py` |
| `signal-replayer.py` | Historical signal replay | `python3 ops/signal-replayer.py` |
| `signal-collector.py` | Real-time signal recording | `python3 ops/signal-collector.py` |
| `generate-daily-report.sh` | Daily HTML reports | `./ops/generate-daily-report.sh <day>` |
| `generate-evaluation-report.py` | Final comprehensive report | `python3 ops/generate-evaluation-report.py` |

### Critical Commands

**Service Management:**
```bash
docker-compose ps                              # Check service status
docker-compose restart <service>              # Restart specific service
docker-compose --profile evaluation down      # Stop evaluation services
```

**Health Checks:**
```bash
curl http://localhost:8080/api/v1/health     # Operator health
curl http://localhost:8081/health             # Scout health
curl http://localhost:9091/-/healthy          # Prometheus health
```

**Database Queries:**
```bash
sqlite3 evaluation/evaluation.db ".tables"   # List database tables
sqlite3 evaluation/evaluation.db \
    "SELECT COUNT(*) FROM evaluation_snapshots;"  # Count snapshots
```

---

## 🔗 Related Documentation

### Main Project Documentation

- **[CLAUDE.md](../../CLAUDE.md)** - Project overview and architecture
- **[README.md](../../README.md)** - Project README and quick start
- **[API Documentation](../../docs/core/api.md)** - REST API reference
- **[Architecture Guide](../../docs/core/architecture.md)** - System architecture details

### Operational Documentation

- **[Runbooks Index](./README.md)** - All operational runbooks
- **[Deployment Guide](../../docs/operations/deployment-guide.md)** - Production deployment
- **[Troubleshooting Guide](../../docs/operations/troubleshooting.md)** - General troubleshooting
- **[Security Guide](../../docs/operations/security-guide.md)** - Security procedures

### Development Documentation

- **[Development Guide](../../docs/development/development-guide.md)** - Development procedures
- **[Testing Guide](../../docs/guides/testing-guide.md)** - Testing procedures
- **[Contributing Guide](../../docs/development/contributing.md)** - Contribution guidelines

---

## 🎯 Evaluation Phases Overview

### Phase 1: Preparation (Day -2 to Day -1)

**Objective**: Set up and validate evaluation infrastructure  
**Key Activities**:
- System requirements validation
- Configuration setup and testing
- Infrastructure preparation
- Pre-evaluation testing

**Documentation Reference**: [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) → "Pre-Evaluation Preparation"

### Phase 2: Startup (Day 0)

**Objective**: Launch evaluation services and begin data collection  
**Key Activities**:
- Start evaluation Docker services
- Verify service health
- Initialize data collection
- Start monitoring systems

**Documentation Reference**: [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) → "Day 0: Evaluation Startup"

### Phase 3: Historical Replay (Days 1-5)

**Objective**: Test system with historical signal replay  
**Key Activities**:
- Monitor signal replay progress
- Daily performance review
- Anomaly detection and response
- Data collection verification

**Documentation Reference**: [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) → "Days 1-5: Historical Replay Phase"

### Phase 4: Real-Time Signals (Days 6-10)

**Objective**: Validate system with real-time signals  
**Key Activities**:
- Transition to real-time signal recording
- Monitor live performance
- Compare real-time vs replay performance
- Final data collection

**Documentation Reference**: [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) → "Days 6-10: Real-Time Signal Phase"

### Phase 5: Analysis (Day 11+)

**Objective**: Generate comprehensive evaluation reports  
**Key Activities**:
- Data validation and quality checks
- Generate comprehensive reports
- Document findings and recommendations
- Archive evaluation data

**Documentation Reference**: [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) → "Post-Evaluation Analysis"

---

## 📞 Support and Contacts

### Evaluation Support

**Technical Support**: [Support contact details]  
**Operational Issues**: [Operations team contact]  
**Emergency Contacts**: See [10-day-evaluation-playbook.md](./10-day-evaluation-playbook.md) → "Emergency Procedures"

### Documentation Issues

**Documentation Maintainer**: [Contact details]  
**Feedback Process**: [GitHub issues or contact details]  
**Update Schedule: Quarterly or as needed

---

## 🔄 Document Maintenance

### Version Control

**Current Version**: 1.0.0  
**Last Updated**: 2026-06-28  
**Next Review**: After evaluation completion

### Change Log

**Version 1.0.0 (2026-06-28)**:
- Initial comprehensive evaluation documentation package
- Complete operational playbook and procedures
- Quick reference and tracking templates
- Post-evaluation reporting template

### Update Process

1. Collect feedback from evaluation execution
2. Identify documentation gaps or improvements
3. Update relevant sections
4. Version control and archiving
5. Distribution to stakeholders

---

## 📈 Evaluation Metrics and Benchmarks

### Key Performance Indicators (KPIs)

**Success Criteria**:
- **Trade Success Rate**: >95%
- **Average Trade Latency**: <100ms
- **System Health Score**: >80/100
- **Data Completeness**: >98%
- **Anomaly Resolution Rate**: >90%

**Grade Scale**:
- **A**: 90-100 (Excellent)
- **B**: 80-89 (Good)
- **C**: 70-79 (Acceptable)
- **D**: 60-69 (Needs Improvement)
- **F**: <60 (Unacceptable)

### Benchmark Comparisons

**Historical Performance**:
- Previous evaluation results
- Industry benchmarks
- Target performance metrics

**Trend Analysis**:
- Day 1 vs Day 10 performance
- Historical vs real-time comparison
- Resource utilization trends

---

## ✅ Documentation Checklist

### For New Evaluations

**Pre-Evaluation**:
- [ ] Read all documentation sections
- [ ] Complete pre-evaluation checklist
- [ ] Set up environment configuration
- [ ] Test infrastructure components
- [ ] Prepare team and procedures

**During Evaluation**:
- [ ] Use checklist for daily tracking
- [ ] Follow operational procedures
- [ ] Document incidents and issues
- [ ] Update evaluation summary template

**Post-Evaluation**:
- [ ] Complete evaluation summary template
- [ ] Generate comprehensive reports
- [ ] Document lessons learned
- [ ] Update documentation based on feedback

---

## 🎓 Training and Onboarding

### New Operator Training

**Phase 1: Orientation** (2 hours)
1. Read evaluation quick reference
2. Review main project documentation
3. Understand evaluation objectives

**Phase 2: Procedures** (4 hours)
1. Study comprehensive playbook
2. Practice daily procedures in dev environment
3. Complete dry-run evaluation

**Phase 3: Certification** (2 hours)
1. Complete pre-evaluation checklist
2. Pass operational quiz
3. Sign-off on procedures

### Refresher Training

**Frequency**: Quarterly or after major updates  
**Topics**: New procedures, lessons learned, updated metrics

---

## 📚 Additional Resources

### External Documentation

- **Docker Documentation**: https://docs.docker.com/
- **Prometheus Documentation**: https://prometheus.io/docs/
- **Grafana Documentation**: https://grafana.com/docs/
- **PostgreSQL Documentation**: https://www.postgresql.org/docs/

### Internal Tools

- **Database Browser**: Use SQLite browser for evaluation.db
- **Log Analysis**: Use evaluation/logs/evaluation/ for log review
- **Monitoring**: Grafana dashboards at http://localhost:3003
- **Metrics**: Prometheus at http://localhost:9091

### Quick Links

- **Start Evaluation**: `./ops/start-evaluation.sh evaluation`
- **Stop Evaluation**: `./ops/emergency-stop-evaluation.sh`
- **Generate Report**: `python3 ops/generate-evaluation-report.py`
- **View Logs**: `tail -f evaluation/logs/evaluation/operator-*.log`

---

**End of Documentation Index**

For questions or issues during evaluation, refer to the specific documentation sections above or contact the evaluation team.