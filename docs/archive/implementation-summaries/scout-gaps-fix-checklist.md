# Scout Module Gaps Fix - Implementation Checklist

**Date Created:** 2025-12-06  
**Status:** 📋 **READY FOR IMPLEMENTATION**

---

## Quick Reference

| Gap | Priority | Effort | Status |
|-----|----------|--------|--------|
| Historical Liquidity Check | 🔴 High | 2 days | ⏳ Pending |
| WQS Base Score Alignment | 🟡 Low | 2-3 hours | ⏳ Pending |
| Enhanced Metric Calculations | 🟠 Medium | 3 days | ⏳ Pending |

---

## Phase 1: Historical Liquidity Infrastructure

### Task 1.1: Enhance LiquidityProvider
- [ ] Add `get_historical_liquidity(token, timestamp, tolerance_hours)` method
- [ ] Add `get_historical_liquidity_or_current(token, timestamp)` method
- [ ] Enhance `_store_in_database()` for batch inserts
- [ ] Add database query logic with timestamp matching
- [ ] Add interpolation logic for timestamps between snapshots
- [ ] Add logging for fallback scenarios
- [ ] Write unit tests for historical lookup
- [ ] Write unit tests for fallback behavior
- [ ] Write unit tests for interpolation
- [ ] Code review

**File:** `scout/core/liquidity.py`  
**Estimated Time:** 4-6 hours

---

### Task 1.2: Update Backtester
- [ ] Change `_simulate_trade()` to use `get_historical_liquidity_or_current()`
- [ ] Pass trade timestamp to liquidity provider
- [ ] Add historical liquidity collection during simulation
- [ ] Add batch insert for liquidity snapshots
- [ ] Add logging when using current liquidity fallback
- [ ] Update method documentation
- [ ] Write integration tests
- [ ] Test with trades at various timestamps
- [ ] Test fallback behavior
- [ ] Code review

**File:** `scout/core/backtester.py`  
**Estimated Time:** 3-4 hours

---

### Task 1.3: Add Collection to Analyzer
- [ ] Add liquidity collection in `get_historical_trades()`
- [ ] Add liquidity collection in `_fetch_real_historical_trades()`
- [ ] Implement batch insert for efficiency
- [ ] Add error handling for collection failures
- [ ] Add logging for collection activity
- [ ] Write tests for collection logic
- [ ] Code review

**File:** `scout/core/analyzer.py`  
**Estimated Time:** 2-3 hours

---

## Phase 2: WQS Base Score Alignment

### Task 2.1: Update WQS Calculation
- [ ] Change base score from `50.0` to `0.0` in `calculate_wqs()`
- [ ] Update function documentation
- [ ] Update module docstring
- [ ] Update all WQS unit tests to expect scores starting from 0
- [ ] Verify test cases still pass
- [ ] Check score distribution is reasonable (0-100 range)
- [ ] Update PDD review document
- [ ] Code review

**File:** `scout/core/wqs.py`  
**Estimated Time:** 2-3 hours

**Optional Enhancement:**
- [ ] Add config flag: `WQS_BASE_SCORE_MODE = "PDD" | "ENHANCED"`
- [ ] Keep both modes for backward compatibility
- [ ] Update tests for both modes

---

## Phase 3: Enhanced Metric Calculations

### Task 3.1: Accurate ROI Calculation
- [ ] Replace `_estimate_roi()` with `_calculate_roi_from_trades()`
- [ ] Implement position tracking logic
- [ ] Implement entry/exit price tracking
- [ ] Implement PnL calculation from price changes
- [ ] Add price history fetching (if needed)
- [ ] Add caching for price data
- [ ] Handle missing price data gracefully
- [ ] Write comprehensive unit tests
- [ ] Test with various trade sequences
- [ ] Test with partial position closes
- [ ] Test with multiple tokens
- [ ] Verify ROI matches manual calculations
- [ ] Code review

**File:** `scout/core/analyzer.py`  
**Estimated Time:** 6-8 hours

---

### Task 3.2: Accurate Win Rate Calculation
- [ ] Replace `_estimate_win_rate()` with `_calculate_win_rate_from_trades()`
- [ ] Use actual PnL from trades
- [ ] Count wins vs losses correctly
- [ ] Handle trades with missing PnL data
- [ ] Write unit tests
- [ ] Test with various win/loss patterns
- [ ] Verify calculation accuracy
- [ ] Code review

**File:** `scout/core/analyzer.py`  
**Estimated Time:** 2-3 hours

---

### Task 3.3: Accurate Drawdown Calculation
- [ ] Replace `_calculate_drawdown_from_trades()` with accurate version
- [ ] Implement running PnL tracking
- [ ] Implement peak identification
- [ ] Implement drawdown calculation: (peak - current) / peak
- [ ] Return maximum drawdown percentage
- [ ] Write unit tests
- [ ] Test with various PnL patterns
- [ ] Test edge cases (all positive, all negative)
- [ ] Verify calculation accuracy
- [ ] Code review

**File:** `scout/core/analyzer.py`  
**Estimated Time:** 3-4 hours

---

### Task 3.4: Accurate Win Streak Consistency
- [ ] Replace `_calculate_win_streak_consistency()` with accurate version
- [ ] Implement streak analysis logic
- [ ] Calculate consistency metric from streak patterns
- [ ] Normalize to 0-1 range
- [ ] Write unit tests
- [ ] Test with various streak patterns
- [ ] Test edge cases
- [ ] Verify calculation accuracy
- [ ] Code review

**File:** `scout/core/analyzer.py`  
**Estimated Time:** 3-4 hours

---

## Testing & Validation

### Unit Tests
- [ ] All historical liquidity tests pass
- [ ] All WQS tests pass (updated for base score 0)
- [ ] All metric calculation tests pass
- [ ] Test coverage > 80% for new code

### Integration Tests
- [ ] Backtester with historical liquidity works end-to-end
- [ ] Wallet analysis with accurate metrics works end-to-end
- [ ] Fallback mechanisms work correctly
- [ ] Performance is acceptable (< 2s per wallet)

### Performance Tests
- [ ] Historical liquidity query performance acceptable
- [ ] Batch insert performance acceptable
- [ ] Cache effectiveness verified
- [ ] No memory leaks

---

## Documentation

### Code Documentation
- [ ] Update docstrings for all changed methods
- [ ] Document historical liquidity lookup logic
- [ ] Document metric calculation formulas
- [ ] Add inline comments for complex logic

### User Documentation
- [ ] Update Scout module documentation
- [ ] Document historical liquidity collection
- [ ] Update PDD compliance status
- [ ] Add migration guide (if needed)

### API Documentation
- [ ] Document new LiquidityProvider methods
- [ ] Update backtester documentation
- [ ] Update analyzer documentation

---

## Deployment Checklist

### Pre-Deployment
- [ ] All tests pass
- [ ] Code review completed
- [ ] Documentation updated
- [ ] Performance benchmarks met
- [ ] Feature flags configured (if using)

### Deployment
- [ ] Deploy Phase 1 (Historical Liquidity)
- [ ] Monitor for errors
- [ ] Verify historical liquidity collection working
- [ ] Deploy Phase 2 (WQS Base Score)
- [ ] Monitor wallet score changes
- [ ] Deploy Phase 3 (Enhanced Metrics)
- [ ] Monitor metric calculation performance
- [ ] Verify all systems operational

### Post-Deployment
- [ ] Monitor error logs
- [ ] Monitor performance metrics
- [ ] Collect user feedback
- [ ] Document any issues
- [ ] Update PDD compliance audit

---

## Rollback Plan

### Historical Liquidity
- [ ] Keep `get_current_liquidity()` method available
- [ ] Can revert backtester to use current liquidity
- [ ] No data loss risk

### WQS Base Score
- [ ] Add config flag for mode selection
- [ ] Can revert via config change
- [ ] No data loss risk

### Enhanced Metrics
- [ ] Keep old methods as fallback
- [ ] Feature flag for gradual rollout
- [ ] Can revert if performance issues

---

## Success Metrics

### Historical Liquidity
- [ ] 90%+ of backtest trades use historical liquidity
- [ ] Fallback rate < 10%
- [ ] Query performance < 100ms per lookup

### WQS Base Score
- [ ] All scores start from 0
- [ ] Score distribution reasonable (0-100)
- [ ] No regression in wallet classification

### Enhanced Metrics
- [ ] ROI accuracy within 1% of manual calculation
- [ ] Win rate accuracy verified
- [ ] Drawdown accuracy verified
- [ ] Performance < 2s per wallet analysis

---

## Notes

### Implementation Order
1. Start with Phase 1 (Historical Liquidity) - highest priority
2. Phase 2 (WQS Base Score) can be done in parallel or after Phase 1
3. Phase 3 (Enhanced Metrics) should be done after Phase 1 is stable

### Dependencies
- Database: `historical_liquidity` table already exists ✅
- External APIs: Helius, Birdeye, Jupiter (may need API keys)
- No breaking changes to existing code

### Risk Mitigation
- Use feature flags for gradual rollout
- Keep old methods as fallback
- Comprehensive testing before deployment
- Monitor performance closely

---

**Last Updated:** 2025-12-06  
**Next Review:** After Phase 1 completion




