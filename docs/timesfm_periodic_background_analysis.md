# TimesFM Periodic Background Prediction Architecture Analysis

## Executive Summary

**Feasibility**: ✅ **VIABLE** - TimesFM 145ms latency can work with periodic background predictions

**Key Finding**: While TimesFM cannot be used for real-time trading signals, it **can** provide value through periodic background forecasting with aggressive caching.

## Architecture Design

### Current Chimera Trading Flow
```
External Signal → Webhook → Validation → Trading Decision (sub-5ms) → Execution
```

### Proposed TimesFM-Enhanced Flow
```
Background: TimesFM (145ms) → SQLite → Cached Predictions
Foreground: Trading Decision → Check Cache → Execution (sub-5ms)
```

## Detailed Architecture

### 1. Background Prediction Worker

**Location**: `/Users/mohammad/Documents/GitHub/chimera/scout/services/timesfm_prediction_worker.py`

**Responsibilities**:
- Fetch price history from `price_cache.rs` (24-hour rolling window)
- Run TimesFM inference (~145ms per token)
- Store predictions in SQLite database
- Update cycle: Every 30 seconds (configurable)

**Performance Characteristics**:
- **Single token**: ~145ms inference
- **10 tokens**: ~1.5s inference
- **20 tokens**: ~3s inference
- **Acceptable**: Yes, since it's background processing

### 2. Prediction Cache & Storage

**Database Schema**:
```sql
CREATE TABLE timesfm_predictions (
    token_address TEXT,
    prediction_time TIMESTAMP,
    horizon_hours INTEGER,
    quantile TEXT,  -- p10, p50, p90
    predicted_price REAL,
    confidence REAL,
    model_version TEXT,
    created_at TIMESTAMP,
    PRIMARY KEY (token_address, prediction_time, horizon_hours, quantile)
);

CREATE INDEX idx_token_time ON timesfm_predictions(token_address, prediction_time);
CREATE INDEX idx_staleness ON timesfm_predictions(created_at);
```

**Cache Strategy**:
- **TTL**: 30 seconds (predictions refresh every 30s)
- **Staleness threshold**: 45 seconds (warn if cache is stale)
- **Fallback**: Use ensemble predictor if TimesFM predictions are stale

### 3. Trading Decision Integration

**Modified Flow**:
```rust
// In profit_targets.rs
fn calculate_profit_target(token: &str) -> Result<Decimal> {
    // 1. Check TimesFM cache (sub-1ms)
    if let Some(prediction) = get_timesfm_prediction(token, 24) {
        let p50 = prediction.p50;
        let p90 = prediction.p90;
        
        // Use p50 for target calculation
        let target = current_price * (1.0 + (p50 - current_price) / current_price * 0.5);
        return Ok(target);
    }
    
    // 2. Fallback to ensemble predictor
    calculate_profit_target_ensemble(token)
}
```

**Performance Impact**:
- **Cache hit**: <1ms (SQLite query with index)
- **Cache miss**: Fallback to ensemble (<50ms)
- **Overall impact**: Negligible (<1ms additional latency)

## Prediction Refresh Strategy

### Option A: Fixed 30-Second Cycle
```
Time 0:00: Predict all 20 tokens (3s)
Time 0:30: Predict all 20 tokens (3s)
Time 1:00: Predict all 20 tokens (3s)
```

**Pros**: Simple, predictable
**Cons**: Wasteful for inactive tokens

### Option B: Priority-Based Caching
```
Active tokens (positions): Every 30s
Watchlist tokens: Every 5 minutes
All other tokens: Every 15 minutes
```

**Pros**: Efficient, resource-optimized
**Cons**: More complex logic

### Option C: Event-Driven Refresh
```
On price change >2%: Refresh prediction immediately
On new position opened: Refresh prediction immediately
On signal received: Refresh prediction immediately
```

**Pros**: Most responsive
**Cons**: Complex, could trigger many updates

## Latency Budget Analysis

### Background Worker Latency
| Operation | Latency | Frequency | Acceptable |
|-----------|---------|-----------|------------|
| Fetch price data | ~5ms | Every 30s | ✅ |
| TimesFM inference | ~145ms | Every 30s | ✅ |
| Database write | ~2ms | Every 30s | ✅ |
| **Total** | **~152ms** | **Every 30s** | **✅** |

### Trading Decision Latency
| Operation | Latency | Impact |
|-----------|---------|--------|
| Cache check | <1ms | Negligible |
| Cache miss fallback | <50ms | Minimal |
| **Total** | **<1ms** | **None** |

## Implementation Complexity

### Low Complexity Components
- Background worker (simple cron job)
- Database schema (straightforward)
- Cache lookup (standard SQLite query)

### Medium Complexity Components
- Priority-based caching logic
- Staleness detection and alerting
- Fallback mechanism

### High Complexity Components
- Event-driven refresh (if chosen)
- Model versioning and rollback
- A/B testing framework

## Use Case Analysis

### ✅ **Suitable Use Cases**

1. **Profit Target Optimization**
   - Predict future prices (24h ahead)
   - Set profit targets based on p50 forecast
   - Refresh every 30s (acceptable latency)

2. **Portfolio Risk Assessment**
   - Predict portfolio-wide price movements
   - Use p90 quantiles for conservative sizing
   - Refresh every 5 minutes (acceptable)

3. **Market Regime Detection**
   - Predict SOL price trends (1-24h ahead)
   - Enhance market regime classifier
   - Refresh every 5 minutes (acceptable)

4. **Exit Planning**
   - Predict optimal exit prices (p50 forecast)
   - Use p10 for stop-loss optimization
   - Refresh every 30s (acceptable)

### ❌ **Unsuitable Use Cases**

1. **Real-time Signal Generation**
   - TimesFM: 145ms
   - Required: <5ms
   - **Gap**: 29x too slow

2. **High-Frequency Trading**
   - TimesFM: 145ms
   - Required: <1ms
   - **Gap**: 145x too slow

3. **Instant Arbitrage**
   - TimesFM: 145ms
   - Required: <10ms
   - **Gap**: 14.5x too slow

## Performance Comparison

### Current System (XGBoost Ensemble)
- **Latency**: <50ms
- **Use case**: Wallet profitability prediction
- **Real-time**: ✅ Yes
- **Price forecasting**: ❌ No

### Proposed TimesFM System
- **Latency**: ~145ms (background), <1ms (cached)
- **Use case**: Token price forecasting
- **Real-time**: ❌ No (but cached results are fast)
- **Price forecasting**: ✅ Yes

## Risk Assessment

### Technical Risks (LOW-MEDIUM)

1. **Stale Predictions** (MEDIUM)
   - **Risk**: Price changes rapidly, predictions become stale
   - **Mitigation**: 30s refresh rate, staleness detection, ensemble fallback
   - **Impact**: Minimal if fallback works correctly

2. **Cache Failures** (LOW)
   - **Risk**: Database issues, cache misses
   - **Mitigation**: Ensemble predictor fallback
   - **Impact**: Minimal (graceful degradation)

3. **Resource Contention** (LOW)
   - **Risk**: Background worker competes for CPU/memory
   - **Mitigation**: Nice/ionice process priority, dedicated thread
   - **Impact**: Minimal (background processing)

### Business Risks (LOW)

1. **Poor Prediction Quality** (LOW)
   - **Risk**: TimesFM predictions worse than current methods
   - **Mitigation**: A/B testing, gradual rollout, automatic rollback
   - **Impact**: Minimal (can disable TimesFM)

2. **Operational Complexity** (LOW)
   - **Risk**: Additional component to monitor and maintain
   - **Mitigation**: Proper monitoring, alerting, runbooks
   - **Impact**: Low (minimal complexity increase)

## Cost-Benefit Analysis

### Costs
- **Development**: 2-3 weeks (simplified from original 9-week plan)
- **Infrastructure**: 2GB RAM, 500MB storage (minimal)
- **Operations**: Monitoring, maintenance, updates (low ongoing cost)
- **Opportunity Cost**: Could invest in improving existing ensemble instead

### Benefits
- **Price Forecasting**: New capability (predict token prices 1-24h ahead)
- **Exit Optimization**: Better profit target calculation
- **Risk Management**: Quantile-based position sizing
- **Market Insights**: Enhanced regime detection
- **Competitive Advantage**: Foundation model technology

### ROI Assessment
- **Investment**: 2-3 weeks development
- **Return**: 5-10% improvement in exit timing (estimated)
- **Payback Period**: 6-12 months
- **Risk-Adjusted ROI**: Positive (low risk, moderate benefit)

## Recommendations

### Option 2A: Minimal Viable Product (MVP)
**Scope**: 2-week implementation
- Background worker with fixed 30s cycle
- Simple cache with SQLite storage
- Integration with profit_targets.rs only
- Basic monitoring and alerting

**Success Criteria**:
- TimesFM predictions available for all tracked tokens
- Cache hit rate >95%
- No degradation in trading latency
- 5% improvement in exit timing

### Option 2B: Full Implementation
**Scope**: 4-5 week implementation
- Priority-based caching (30s/5min/15min)
- Integration with profit_targets, stop_loss, position_sizer
- Comprehensive monitoring and A/B testing
- Event-driven refresh for significant price changes

**Success Criteria**:
- All MVP criteria PLUS
- 10% improvement in overall trading profitability
- Quantile-based risk management validated
- A/B test shows significant improvement vs baseline

## Decision Matrix

| Factor | Current Ensemble | TimesFM Background |
|--------|-----------------|-------------------|
| Latency | <50ms ✅ | <1ms cached ✅ |
| Price Forecasting | ❌ No | ✅ Yes |
| Real-time Capable | ✅ Yes | ❌ No (but fast cache) |
| Development Time | 0 weeks | 2-5 weeks |
| Risk | Low | Low-Medium |
| Expected Benefit | Baseline | +5-10% |

## Conclusion

**Feasibility**: ✅ **Feasible with architectural changes**

**Recommendation**: **Proceed with Option 2A (MVP)** - Minimal viable implementation with 2-week scope.

**Key Success Factor**: Accept that TimesFM is for **periodic background predictions**, not real-time trading signals.

**Next Steps**:
1. Implement MVP background worker (1 week)
2. Integrate with profit_targets (1 week)  
3. A/B test against baseline (2 weeks)
4. Evaluate and decide on full implementation

**Go/No-Go Decision**: If MVP shows >5% improvement in exit timing, proceed to full implementation (Option 2B). Otherwise, revert to ensemble-only approach.