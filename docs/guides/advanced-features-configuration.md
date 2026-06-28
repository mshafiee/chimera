# Advanced Features Configuration Guide

## Overview

Chimera includes sophisticated intelligence features that enhance wallet analysis, risk management, and position sizing. This guide explains how to enable and configure these features for production deployment.

## Feature Categories

### 1. Wallet Intelligence Features (Scout)

These features enhance the Scout wallet analysis pipeline with advanced analytics:

#### **Advanced Risk Features** ✅ ENABLED
**Purpose**: Sophisticated risk analysis beyond basic drawdown metrics

**Capabilities**:
- CVaR (Conditional Value at Risk) at 90%, 95%, 99% confidence levels
- Tail risk analysis and Ulcer Index calculations
- Maximum drawdown duration tracking
- Risk regime classification (high/low volatility)
- Enhanced wallet notes with risk insights

**Environment Variables**:
```bash
SCOUT_ADVANCED_RISK_FEATURES=true  # Default: true
```

**Requirements**: Minimum 5 historical trades per wallet for reliable analysis

**Benefits**:
- Better risk-adjusted wallet selection
- Early warning for high-risk wallets
- Improved position sizing decisions

---

#### **Time Series Features** ✅ ENABLED
**Purpose**: Temporal pattern analysis for predictive insights

**Capabilities**:
- RSI (Relative Strength Index) for overbought/oversold detection
- MACD (Moving Average Convergence Divergence) trend analysis
- Bollinger Bands for volatility measurement
- Momentum scoring and autocorrelation
- Performance persistence detection

**Environment Variables**:
```bash
SCOUT_TIME_SERIES_FEATURES=true  # Default: true
```

**Requirements**: Minimum 3 historical trades with timestamps

**Benefits**:
- Identifies momentum shifts before they impact performance
- Detects mean-reverting vs trending patterns
- Improves entry/exit timing signals

---

#### **Network Features** ✅ ENABLED
**Purpose**: Graph-based wallet intelligence for sybil detection and relationship analysis

**Capabilities**:
- PageRank centrality scoring for influence measurement
- Sybil cluster detection via shared funding patterns
- Token co-holding analysis for correlation detection
- Community detection and network clustering
- Cross-wallet relationship mapping

**Environment Variables**:
```bash
SCOUT_NETWORK_FEATURES=true  # Default: true
```

**Requirements**: Requires Helius API for funder relationship analysis

**Benefits**:
- Prevents sybil wallets from dominating the roster
- Identifies correlated wallet groups
- Improves roster diversity and reduces concentration risk

---

### 2. Roster Diversity Features (Scout)

#### **Wallet Clustering & Deduplication** ✅ ENABLED
**Purpose**: Prevents correlated risk by grouping wallets from the same funder

**Capabilities**:
- Funder-based cluster detection via Helius API
- Top-WQS wallet selection per cluster
- Singleton handling for wallets without known funders
- Cluster ID assignment for tracking

**Environment Variables**:
```bash
SCOUT_CLUSTER_DEDUP=true  # Default: true
```

**Behavior**: Demotes lower-WQS wallets from the same funder to CANDIDATE status

**Benefits**:
- Prevents single trader's multiple wallets from dominating the roster
- Reduces correlated risk from sybil attacks
- Ensures diverse signal sources

---

#### **Cluster Ensemble Scoring** ✅ ENABLED
**Purpose**: Penalizes wallets in underperforming clusters to ensure quality

**Capabilities**:
- Cluster-wide performance metrics (ROI, profit factor)
- WQS adjustment based on cluster performance
- Losing cluster detection and penalty application
- Ensemble-based scoring refinement

**Environment Variables**:
```bash
SCOUT_CLUSTER_ENSEMBLE=true  # Default: true
```

**Behavior**: Adjusts individual wallet WQS based on cluster performance

**Benefits**:
- Improves overall roster quality
- Reduces impact of systematic underperformance
- Encourages diverse, high-quality clusters

---

#### **Cross-Wallet Token Correlation** ✅ ENABLED
**Purpose**: Prevents portfolio concentration risk by detecting token overlap

**Capabilities**:
- Token overlap analysis between wallet portfolios
- configurable overlap thresholds (default: 70%)
- Enhanced sybil detection via shared funder + token overlap
- Lower-WQS wallet demotion for correlated portfolios

**Environment Variables**:
```bash
SCOUT_CROSS_WALLET_CORRELATION=true  # Default: true
```

**Behavior**: Demotes wallets with >70% token overlap to CANDIDATE status

**Benefits**:
- Prevents correlated risk across the roster
- Ensures diverse token exposure
- Improves risk management through portfolio diversification

---

### 3. Position Sizing Features (Operator)

#### **Kelly Criterion Position Sizing** ✅ ENABLED
**Purpose**: Mathematical position sizing for optimal capital growth

**Capabilities**:
- Edge/odds Kelly formula: k = (p*b - q) / b
- Conservative Kelly fraction (25%) with safety caps
- Adaptive lookback periods (14d vs 30d)
- Velocity multipliers for high-frequency traders
- Fallback to WQS-based sizing for insufficient history

**Configuration**:
```yaml
position_sizing:
  use_kelly_sizing: true
  total_capital_sol: 10.0  # Adjust to actual trading capital
  kelly_fraction: 0.25    # Conservative 25% of full Kelly
```

**Requirements**: Minimum 15 closed trades for reliable Kelly calculation

**Benefits**:
- Optimal position sizing for long-term growth
- Automatic risk adjustment based on performance
- Mathematical foundation for capital allocation
- Prevents over-leveraging on high-risk signals

---

## Production Configuration

### Docker Compose Configuration

Update your `docker-compose.prod.yml` environment variables:

```yaml
services:
  scout:
    environment:
      # Advanced Intelligence Features (All Enabled)
      - SCOUT_CLUSTER_DEDUP=true
      - SCOUT_CLUSTER_ENSEMBLE=true
      - SCOUT_CROSS_WALLET_CORRELATION=true
      - SCOUT_ADVANCED_RISK_FEATURES=true
      - SCOUT_TIME_SERIES_FEATURES=true
      - SCOUT_NETWORK_FEATURES=true
```

### Operator Configuration

Update your `config/config.yaml`:

```yaml
position_sizing:
  use_kelly_sizing: true
  total_capital_sol: 10.0  # Adjust to your trading capital
  kelly_fraction: 0.25    # Conservative Kelly fraction
  base_size_sol: 0.1
  max_size_sol: 2.0
  min_size_sol: 0.02
```

### Environment File Template

Create `.env` file for Scout:

```bash
# Advanced Features (Production Settings)
SCOUT_CLUSTER_DEDUP=true
SCOUT_CLUSTER_ENSEMBLE=true
SCOUT_CROSS_WALLET_CORRELATION=true
SCOUT_ADVANCED_RISK_FEATURES=true
SCOUT_TIME_SERIES_FEATURES=true
SCOUT_NETWORK_FEATURES=true

# Performance Thresholds
SCOUT_MIN_WQS_ACTIVE=60.0
SCOUT_MIN_WQS_CANDIDATE=30.0
SCOUT_MIN_CLOSES_REQUIRED=10
SCOUT_MAX_WALLETS=250

# API Configuration
HELIUS_API_KEY=your-helius-api-key
```

---

## Feature Dependencies

### Required Services

1. **Redis Server**: Required for Scout caching and performance
   ```bash
   docker-compose up -d redis
   ```

2. **Helius API**: Required for network features and funder analysis
   ```bash
   HELIUS_API_KEY=your-api-key
   ```

3. **Database**: Must have at least 15 historical trades for Kelly Criterion

### Optional Services

- **Birdeye API**: Enhanced liquidity data (optional)
- **Jupiter API**: DEX routing and slippage estimation

---

## Performance Impact

### Computational Requirements

- **Network Features**: +5-10% processing time per wallet
- **Time Series Features**: +3-5% processing time per wallet
- **Advanced Risk Features**: +2-3% processing time per wallet
- **Clustering Features**: +10-15% total processing time

### Memory Requirements

- **Base Scout Memory**: ~200MB
- **With All Features**: ~350-400MB
- **Recommendation**: Minimum 512MB RAM for Scout container

---

## Monitoring & Validation

### Enable Feature Logging

All advanced features log their activity:

```bash
# Scout logs show feature activation
tail -f logs/scout.log | grep -E "(Network|Time-series|Advanced Risk|Clustering)"

# Operator logs show Kelly Criterion calculations
tail -f logs/operator.log | grep -E "(Kelly|position sizing)"
```

### Health Check Endpoints

Verify feature status via API:

```bash
# Scout feature status (if available)
curl http://localhost:8080/api/v1/scout/status

# Operator position sizing configuration
curl http://localhost:8080/api/v1/operations/resources | jq .position_sizing
```

### Performance Metrics

Monitor feature impact:

```bash
# Scout processing time
curl http://localhost:8080/metrics | grep scout_processing_duration_seconds

# Kelly Criterion calculations
curl http://localhost:8080/metrics | grep kelly_calculation_duration_seconds
```

---

## Troubleshooting

### Feature Not Working

1. **Check Environment Variables**:
   ```bash
   docker-compose config | grep SCOUT_
   ```

2. **Verify Dependencies**:
   ```bash
   # Check Redis connectivity
   docker-compose exec scout redis-cli -h redis ping

   # Check Helius API key
   curl -H "Authorization: Bearer $HELIUS_API_KEY" https://api.helius.xyz/v0/health
   ```

3. **Review Logs**:
   ```bash
   docker-compose logs scout | grep -E "(ERROR|WARNING|feature)"
   ```

### Kelly Criterion Not sizing

1. **Verify Configuration**:
   ```bash
   grep -A 5 "position_sizing:" config/config.yaml
   ```

2. **Check Trade History**:
   ```bash
   # Query database for trade count
   sqlite3 data/chimera.db "SELECT wallet_address, COUNT(*) FROM trades WHERE status='CLOSED' GROUP BY wallet_address;"
   ```

3. **Review Kelly Logs**:
   ```bash
   docker-compose logs operator | grep -i kelly
   ```

---

## Safety Considerations

### Conservative Defaults

All features use conservative defaults for production safety:

- **Kelly Criterion**: 25% of full Kelly (prevents over-leveraging)
- **Token Correlation**: 70% threshold (prevents false positives)
- **Cluster Detection**: Top-WQS per cluster (prevents signal loss)
- **Risk Features**: Minimum 5-15 trades required (prevents unreliable data)

### Gradual Rollout

For new deployments, enable features incrementally:

1. **Week 1**: Enable basic features (clustering, correlation)
2. **Week 2**: Add advanced risk features
3. **Week 3**: Enable time-series and network features
4. **Week 4**: Enable Kelly Criterion position sizing

### Rollback Procedures

To disable specific features:

```bash
# Disable individual features
SCOUT_ADVANCED_RISK_FEATURES=false
SCOUT_NETWORK_FEATURES=false

# Disable Kelly Criterion
# Edit config.yaml: use_kelly_sizing: false
```

---

## Best Practices

### 1. Regular Feature Review

Review feature impact weekly:

```bash
# Check wallet quality metrics
curl http://localhost:8080/api/v1/wallets?status=ACTIVE | jq '.[] | {address, wqs_score, notes}'

# Check position sizing effectiveness
curl http://localhost:8080/api/v1/positions | jq '.[] | {wallet_address, size_sol, pnl_sol}'
```

### 2. Performance Monitoring

Monitor resource usage:

```bash
# Scout container metrics
docker stats chimera-scout

# Processing time trends
curl http://localhost:8080/metrics | grep scout_duration
```

### 3. Quality Validation

Validate feature effectiveness:

```bash
# Compare WQS scores vs actual performance
curl http://localhost:8080/api/v1/correlation/stats

# Check cluster diversity
curl http://localhost:8080/api/v1/wallets?status=ACTIVE | jq 'length'  # Should be 15-25 diverse wallets
```

---

## Summary

Chimera's advanced features provide sophisticated wallet intelligence, risk management, and position sizing capabilities. All features are:

- ✅ **Production-ready**: Thoroughly tested and validated
- ✅ **Conservative**: Safe defaults prevent over-optimization
- ✅ **Configurable**: Easy to enable/disable via environment variables
- ✅ **Monitored**: Comprehensive logging and metrics
- ✅ **Documented**: Full operational guidance

Enable these features to transform Chimera from a basic copy-trading platform into a sophisticated, intelligence-driven trading system with mathematical foundation and advanced risk management.