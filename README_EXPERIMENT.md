# 21-Day Live Forward Test - Quick Start Guide

## Overview
This infrastructure enables a 21-day live forward test with 0.02 SOL tracer trades, control arms, and statistical verdict evaluation to prove/disprove profitability on a $49 Helius Developer Plan.

## Prerequisites
- Helius Developer API key
- Valid vault configuration with wallet keypair
- Operator compiled with experiment modules
- SQLite database with experiment schema applied

## Quick Start

### 1. Setup Experiment (One-time)
```bash
# Initialize experiment with T0 selection and roster freeze
./scout/scripts/setup_experiment.py --config config/experiment.yaml

# Verify experiment manifest created
sqlite3 operator/data/chimera.db "SELECT * FROM experiment_manifest;"
```

### 2. Run Validation Test
```bash
# Validate all components before production
./scout/scripts/run_shakedown_test.sh
```

### 3. Start Real Experiment
```bash
# Automated orchestration with monitoring
./run-forward-test.sh

# Manual start with experiment mode
operator/target/release/chimera_operator --config config/experiment.yaml --mode live --experiment-enabled
```

### 4. Monitor Progress
```bash
# Grafana Dashboard
open http://localhost:3000/d/chimera-experiment-dashboard

# Prometheus Metrics
curl http://localhost:8080/metrics | grep chimera_experiment

# Database Status
sqlite3 operator/data/chimera.db "SELECT status, elapsed_days, total_trades, tracer_count FROM experiment_manifest;"
```

### 5. Generate Verdict
```bash
# After 21 days or 50 trades, run verdict analysis
python scout/scripts/verdict.py --db-path operator/data/chimera.db

# Expected output: GO, KILL, or INCONCLUSIVE with detailed statistics
```

## Key Components

### Core Modules (`operator/src/experiment/`)
- `tracer.rs` - Tracer execution hook with sample rate tapering
- `controls.rs` - Random-token and SOL benchmark control arms
- `ledger.rs` - Paper-only vs paper+tracer separation
- `verdict.rs` - BCa bootstrap confidence interval evaluation
- `toxic.rs` - Toxic flow detection and wallet blacklisting

### Python Scripts (`scout/scripts/`)
- `setup_experiment.py` - Experiment initialization with T0 selection
- `verdict.py` - Statistical verdict generation with GO/KILL rules
- `run_shakedown_test.sh` - Component validation and testing

### Configuration (`config/experiment.yaml`)
```yaml
tracer_enabled: true          # Enable live tracer trades
tracer_sample_rate: 1.0       # Start at 100% sample rate
tracer_cap: 60                # Max 60 tracer trades
experiment_days: 21           # Minimum experiment duration
min_trades: 50                # Minimum trades for verdict
controls_enabled: true        # Enable control arms
toxic_threshold_percent: 30   # Toxic wallet kill threshold
```

## Risk Management

### Capital Exposure
- **Tracer Size**: 0.02 SOL per trade
- **Peak Exposure**: 0.1 SOL (5 concurrent positions × 0.02 SOL)
- **Total Budget**: 1.2 SOL (60 trades × 0.02 SOL)

### Abort Conditions
- Credit exhaustion (<10% remaining)
- Toxic wallet rate (>30%)
- Max drawdown (>20%)
- Circuit breaker trips
- Verdict generation

### Statistical Validity
- Minimum 50 trades required
- BCa bootstrap confidence intervals
- Control arms for baseline comparison
- Anti-look-ahead T0 selection

## Monitoring Dashboard

### Grafana Panels
- Experiment status and progress
- Trade counts and PnL metrics
- Execution gap analysis
- Toxic flow detection
- Credit budget tracking
- Performance latencies
- Abort condition monitoring

### Key Metrics
- `chimera_experiment_status` - Current experiment state
- `chimera_experiment_total_trades` - Total trades recorded
- `chimera_experiment_tracer_count` - Live tracer trades
- `chimera_experiment_total_pnl` - Overall profitability
- `chimera_experiment_toxic_wallet_count` - Toxic wallets detected

## Troubleshooting

### Common Issues

**No trades appearing in experiment_trades**
- Verify `tracer_enabled: true` in config
- Check experiment_manifest created with proper T0 timestamp
- Ensure operator is running with experiment mode enabled

**Abort condition triggers early**
- Review toxic_threshold_percent setting (default 30%)
- Check max_drawdown threshold (default 20%)
- Verify credit budget is sufficient

**Verdict script returns INCONCLUSIVE**
- Ensure minimum 50 trades recorded
- Check experiment has run for at least 21 days
- Verify BCa bootstrap can compute confidence intervals

**Grafana dashboard shows no data**
- Confirm Prometheus metrics endpoint accessible
- Check experiment status is 'running' in database
- Verify metrics collection interval (default 10 seconds)

### Recovery Commands

```bash
# Reset experiment (delete all data)
sqlite3 operator/data/chimera.db "DELETE FROM experiment_manifest; DELETE FROM experiment_trades; DELETE FROM toxic_wallets;"

# Restart aborted experiment
sqlite3 operator/data/chimera.db "UPDATE experiment_manifest SET status = 'running', abort_reason = NULL WHERE id = 1;"

# Clear toxic wallets (for testing)
sqlite3 operator/data/chimera.db "DELETE FROM toxic_wallets;"
```

## Decision Rules

### GO Criteria
- Expectancy > 0 with lower CI > 0
- Profit Factor > 1.2
- Beats both control arms
- Max drawdown < 20%
- Toxic rate < 30%

### KILL Criteria
- Any abort condition triggered
- Expectancy CI includes 0
- Insufficient statistical power

### INCONCLUSIVE Criteria
- Insufficient data (<50 trades or <21 days)
- High variance with wide CIs
- External interference

## Technical Specifications

### Helius API Usage
- **Plan**: Developer ($49/month)
- **Credits**: 10M/month, 50 RPS, 5 sendTransaction/s
- **Estimated Usage**: 39,540 credits over 21 days (0.56%)
- **Safety Margin**: Massive headroom for retries and errors

### Database Schema
- `experiment_trades` - Individual trade records
- `experiment_manifest` - Experiment metadata and status
- `toxic_wallets` - Flagged wallet addresses
- `experiment_credits` - Credit usage tracking

### Performance Targets
- Execution gap < 5 seconds
- Trade recording latency < 1 second
- Toxic detection within 24 hours
- Abort condition response < 10 seconds

## Next Steps

1. **Complete vault setup** with valid wallet keypair
2. **Run setup_experiment.py** to initialize experiment
3. **Verify experiment_manifest** created with proper T0 timestamp
4. **Start operator** with experiment configuration
5. **Monitor Grafana dashboard** for initial activity
6. **Check abort conditions** in first 24 hours
7. **Prepare verdict analysis** after 21 days

## Support

- **Documentation**: `docs/FORWARD_TEST_COMPLETION.md`
- **Integration Summary**: `docs/INTEGRATION_IMPLEMENTATION.md`
- **Shake-Down Test**: `scout/scripts/run_shakedown_test.sh`
- **Verdict Analysis**: `scout/scripts/verdict.py`

## Status

✅ **Implementation Complete** - All components integrated and validated  
✅ **Shake-Down Test Passed** - Production-ready infrastructure  
✅ **Documentation Complete** - Comprehensive guides and troubleshooting  

**Ready for production deployment upon vault setup completion.**
