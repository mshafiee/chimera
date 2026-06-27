# Scout Module Gaps Fix Plan

**Date:** 2025-12-06  
**PDD Version:** 7.1 (Engineering Freeze)  
**Status:** 📋 **PLANNING**

---

## Executive Summary

This document outlines a comprehensive plan to fix all identified gaps in the Scout module to achieve 100% PDD compliance. The plan addresses three main areas:

1. **Historical Liquidity Check** - Implement true historical liquidity validation
2. **WQS Base Score Alignment** - Align with PDD specification (score starts at 0)
3. **Enhanced Metric Calculations** - Implement accurate ROI, win rate, and drawdown calculations

**Estimated Effort:** 3-5 days  
**Priority:** High (for full PDD compliance)  
**Risk Level:** Low-Medium (well-defined changes)

---

## Gap Analysis

### Gap 1: Historical Liquidity Check ⚠️ **HIGH PRIORITY**

**Current State:**
- Backtester uses `get_current_liquidity()` for all trades
- PDD requires checking liquidity **at the time of each historical trade**
- Database schema already has `historical_liquidity` table (good foundation)

**Impact:**
- Medium - Current implementation is functional but not fully compliant
- May incorrectly validate wallets that traded when liquidity was higher

**Root Cause:**
- No historical liquidity lookup method in `LiquidityProvider`
- Backtester doesn't query historical liquidity database
- No historical liquidity collection during trade analysis

---

### Gap 2: WQS Base Score Alignment ⚠️ **LOW PRIORITY**

**Current State:**
- WQS calculation starts with `score = 50.0` (neutral baseline)
- PDD specification shows `score = 0` initially

**Impact:**
- Low - Functionally equivalent, just different baseline
- May slightly affect score distribution

**Root Cause:**
- Design choice to use neutral baseline instead of zero-based

---

### Gap 3: Enhanced Metric Calculations ⚠️ **MEDIUM PRIORITY**

**Current State:**
- ROI estimation is simplified (uses trade frequency as proxy)
- Win rate is hardcoded to 0.6
- Drawdown calculation is simplified
- Win streak consistency is simplified

**Impact:**
- Medium - Metrics may not accurately reflect wallet performance
- Could lead to incorrect WQS scores

**Root Cause:**
- Missing price history data for accurate PnL calculation
- Simplified implementations for development/testing

---

## Implementation Plan

### Phase 1: Historical Liquidity Infrastructure (Day 1-2)

#### Task 1.1: Enhance LiquidityProvider with Historical Lookup

**File:** `scout/core/liquidity.py`

**Changes:**
1. Add `get_historical_liquidity()` method that:
   - Queries `historical_liquidity` table for closest timestamp
   - Falls back to current liquidity if no historical data found
   - Uses interpolation for timestamps between snapshots
   - Returns `LiquidityData` with historical timestamp

2. Add `get_historical_liquidity_or_current()` method:
   - Tries historical lookup first
   - Falls back to current liquidity if historical unavailable
   - Logs fallback for monitoring

3. Enhance `_store_in_database()` to:
   - Store liquidity snapshots during trade analysis
   - Batch insert for efficiency
   - Handle duplicate timestamps gracefully

**Code Structure:**
```python
def get_historical_liquidity(
    self,
    token_address: str,
    timestamp: datetime,
    tolerance_hours: int = 6,  # Accept data within 6 hours
) -> Optional[LiquidityData]:
    """
    Get historical liquidity for a token at a specific timestamp.
    
    Queries the historical_liquidity table for the closest snapshot
    to the requested timestamp.
    
    Args:
        token_address: Token mint address
        timestamp: Historical timestamp
        tolerance_hours: Maximum time difference to accept (default 6 hours)
        
    Returns:
        LiquidityData or None if not available
    """
    # Implementation: Query database for closest timestamp
    # Use SQL: SELECT * FROM historical_liquidity 
    # WHERE token_address = ? AND timestamp <= ? 
    # ORDER BY timestamp DESC LIMIT 1
    # Then check if within tolerance
    pass

def get_historical_liquidity_or_current(
    self,
    token_address: str,
    timestamp: datetime,
) -> Optional[LiquidityData]:
    """
    Get historical liquidity, falling back to current if unavailable.
    
    This is the primary method for backtesting - it ensures we always
    have liquidity data, even if historical data is missing.
    """
    historical = self.get_historical_liquidity(token_address, timestamp)
    if historical:
        return historical
    
    # Fallback to current liquidity
    current = self.get_current_liquidity(token_address)
    if current:
        # Create historical data point from current
        return LiquidityData(
            token_address=current.token_address,
            liquidity_usd=current.liquidity_usd,
            price_usd=current.price_usd,
            volume_24h_usd=current.volume_24h_usd,
            timestamp=timestamp,  # Use historical timestamp
            source=f"{current.source}_fallback",
        )
    return None
```

**Testing:**
- Unit tests for historical lookup with various timestamps
- Test fallback behavior when historical data missing
- Test interpolation logic
- Integration test with real database

---

#### Task 1.2: Update Backtester to Use Historical Liquidity

**File:** `scout/core/backtester.py`

**Changes:**
1. Modify `_simulate_trade()` method:
   - Change from `get_current_liquidity()` to `get_historical_liquidity_or_current()`
   - Pass trade timestamp to liquidity provider
   - Log when using current liquidity as fallback

2. Add historical liquidity collection:
   - During trade simulation, collect liquidity snapshots
   - Store in database for future use
   - Batch insert for efficiency

**Code Changes:**
```python
# In _simulate_trade method, line ~203:
# OLD:
liquidity_data = self.liquidity.get_current_liquidity(trade.token_address)

# NEW:
liquidity_data = self.liquidity.get_historical_liquidity_or_current(
    trade.token_address,
    trade.timestamp,  # Use historical timestamp
)

# Also collect current liquidity snapshot for future historical queries
if liquidity_data:
    self.liquidity._store_in_database(liquidity_data)
```

**Testing:**
- Test backtester with trades at various timestamps
- Verify historical liquidity is used when available
- Verify fallback to current liquidity works
- Test with trades spanning multiple days

---

#### Task 1.3: Add Historical Liquidity Collection to Analyzer

**File:** `scout/core/analyzer.py`

**Changes:**
1. When fetching historical trades, collect liquidity snapshots:
   - For each trade, fetch liquidity at trade timestamp
   - Store in `historical_liquidity` table
   - Batch insert for efficiency

2. Add background task (optional):
   - Periodically collect liquidity snapshots for tracked tokens
   - Can be run via cron or as part of Scout main loop

**Code Changes:**
```python
# In _fetch_real_historical_trades or get_historical_trades:
# After parsing each trade, collect liquidity snapshot
for trade in trades:
    # Collect liquidity at trade time
    liquidity = self.liquidity_provider.get_current_liquidity(trade.token_address)
    if liquidity:
        # Store with trade timestamp
        historical_liq = LiquidityData(
            token_address=liquidity.token_address,
            liquidity_usd=liquidity.liquidity_usd,
            price_usd=liquidity.price_usd,
            volume_24h_usd=liquidity.volume_24h_usd,
            timestamp=trade.timestamp,  # Use trade timestamp
            source="analyzer_collection",
        )
        self.liquidity_provider._store_in_database(historical_liq)
```

**Testing:**
- Test liquidity collection during trade analysis
- Verify data is stored correctly
- Test batch insert performance

---

### Phase 2: WQS Base Score Alignment (Day 2, 2-3 hours)

#### Task 2.1: Update WQS Calculation to Start at 0

**File:** `scout/core/wqs.py`

**Changes:**
1. Change base score from `50.0` to `0.0`
2. Update documentation to reflect PDD specification
3. Keep activity bonus (it's an enhancement, not a gap)

**Code Changes:**
```python
# Line 54:
# OLD:
score = 50.0

# NEW:
score = 0.0  # PDD specification: start at 0
```

**Testing:**
- Update existing WQS tests to expect scores starting from 0
- Verify all test cases still pass
- Check that score distribution is reasonable (0-100 range)

**Note:** This is a simple change but may affect existing wallet scores. Consider:
- Documenting the change in release notes
- Option to keep both modes (PDD mode vs enhanced mode) via config flag

---

### Phase 3: Enhanced Metric Calculations (Day 3-5)

#### Task 3.1: Implement Accurate ROI Calculation

**File:** `scout/core/analyzer.py`

**Changes:**
1. Replace `_estimate_roi()` with accurate calculation:
   - Track entry prices for each token position
   - Calculate PnL from actual price changes
   - Sum PnL across all closed positions
   - Divide by total capital deployed

2. Add price history fetching:
   - Use Helius API or Birdeye for historical prices
   - Cache price data to avoid redundant API calls
   - Handle missing price data gracefully

**Code Structure:**
```python
def _calculate_roi_from_trades(
    self,
    trades: List[HistoricalTrade],
    days: int = 30,
) -> float:
    """
    Calculate accurate ROI from historical trades.
    
    Tracks positions and calculates PnL from actual price changes.
    
    Args:
        trades: List of historical trades
        days: Time window for ROI calculation
        
    Returns:
        ROI as percentage
    """
    # Track positions: {token_address: {entry_price, entry_amount, ...}}
    positions = {}
    total_capital = 0.0
    total_pnl = 0.0
    
    # Sort trades chronologically
    sorted_trades = sorted(trades, key=lambda t: t.timestamp)
    
    for trade in sorted_trades:
        if trade.action == TradeAction.BUY:
            # Open or add to position
            if trade.token_address not in positions:
                positions[trade.token_address] = {
                    'entry_price': trade.price_at_trade,
                    'entry_amount': trade.amount_sol,
                    'total_cost': trade.amount_sol * trade.price_at_trade,
                }
            else:
                # Average entry price
                pos = positions[trade.token_address]
                total_cost = pos['total_cost'] + (trade.amount_sol * trade.price_at_trade)
                total_amount = pos['entry_amount'] + trade.amount_sol
                pos['entry_price'] = total_cost / total_amount
                pos['entry_amount'] = total_amount
                pos['total_cost'] = total_cost
            
            total_capital += trade.amount_sol * trade.price_at_trade
            
        elif trade.action == TradeAction.SELL:
            # Close position and calculate PnL
            if trade.token_address in positions:
                pos = positions[trade.token_address]
                entry_price = pos['entry_price']
                exit_price = trade.price_at_trade
                
                # Calculate PnL
                pnl = (exit_price - entry_price) * min(trade.amount_sol, pos['entry_amount'])
                total_pnl += pnl
                
                # Update position
                pos['entry_amount'] -= trade.amount_sol
                if pos['entry_amount'] <= 0:
                    del positions[trade.token_address]
    
    # Calculate ROI
    if total_capital <= 0:
        return 0.0
    
    roi_percent = (total_pnl / total_capital) * 100
    return roi_percent
```

**Testing:**
- Test with various trade sequences (buy/sell patterns)
- Test with partial position closes
- Test with multiple tokens
- Verify ROI matches manual calculations

---

#### Task 3.2: Implement Accurate Win Rate Calculation

**File:** `scout/core/analyzer.py`

**Changes:**
1. Replace `_estimate_win_rate()` with accurate calculation:
   - Use actual PnL from trades (from ROI calculation)
   - Count winning trades (PnL > 0) vs losing trades (PnL < 0)
   - Calculate win_rate = wins / total_trades

**Code Structure:**
```python
def _calculate_win_rate_from_trades(
    self,
    trades: List[HistoricalTrade],
) -> float:
    """
    Calculate accurate win rate from historical trades.
    
    Uses actual PnL data to determine wins vs losses.
    
    Args:
        trades: List of historical trades
        
    Returns:
        Win rate as float (0.0 to 1.0)
    """
    if not trades:
        return 0.0
    
    # Only count SELL trades (closing positions) for win/loss
    closing_trades = [t for t in trades if t.action == TradeAction.SELL]
    
    if not closing_trades:
        return 0.0
    
    wins = sum(1 for t in closing_trades if t.pnl_sol and t.pnl_sol > 0)
    losses = sum(1 for t in closing_trades if t.pnl_sol and t.pnl_sol < 0)
    total = wins + losses
    
    if total == 0:
        return 0.0
    
    return wins / total
```

**Testing:**
- Test with various win/loss patterns
- Test with trades missing PnL data
- Verify win rate calculation accuracy

---

#### Task 3.3: Implement Accurate Drawdown Calculation

**File:** `scout/core/analyzer.py`

**Changes:**
1. Replace `_calculate_drawdown_from_trades()` with accurate calculation:
   - Track running PnL over time
   - Identify peak values
   - Calculate drawdown from peak: (peak - current) / peak
   - Return maximum drawdown percentage

**Code Structure:**
```python
def _calculate_drawdown_from_trades(
    self,
    trades: List[HistoricalTrade],
) -> float:
    """
    Calculate maximum drawdown from historical trades.
    
    Tracks running PnL and identifies peak-to-trough declines.
    
    Args:
        trades: List of historical trades
        
    Returns:
        Maximum drawdown as percentage (0.0 to 100.0)
    """
    if not trades:
        return 0.0
    
    # Sort trades chronologically
    sorted_trades = sorted(trades, key=lambda t: t.timestamp)
    
    # Track running PnL
    running_pnl = 0.0
    peak_pnl = 0.0
    max_drawdown = 0.0
    
    for trade in sorted_trades:
        # Update running PnL
        if trade.pnl_sol:
            running_pnl += trade.pnl_sol
        
        # Update peak
        if running_pnl > peak_pnl:
            peak_pnl = running_pnl
        
        # Calculate drawdown from peak
        if peak_pnl > 0:
            drawdown = (peak_pnl - running_pnl) / peak_pnl
            max_drawdown = max(max_drawdown, drawdown)
    
    return max_drawdown * 100  # Convert to percentage
```

**Testing:**
- Test with various PnL patterns
- Test with all-positive PnL (should return 0% drawdown)
- Test with all-negative PnL
- Verify drawdown calculation accuracy

---

#### Task 3.4: Implement Accurate Win Streak Consistency

**File:** `scout/core/analyzer.py`

**Changes:**
1. Replace `_calculate_win_streak_consistency()` with accurate calculation:
   - Analyze actual win/loss streaks from trades
   - Calculate consistency metric based on streak patterns
   - Higher consistency = more regular win patterns

**Code Structure:**
```python
def _calculate_win_streak_consistency(
    self,
    trades: List[HistoricalTrade],
) -> float:
    """
    Calculate win streak consistency from historical trades.
    
    Analyzes win/loss patterns to determine consistency.
    Higher value = more consistent winning patterns.
    
    Args:
        trades: List of historical trades
        
    Returns:
        Consistency score (0.0 to 1.0)
    """
    if not trades:
        return 0.0
    
    # Get closing trades with PnL
    closing_trades = [
        t for t in trades 
        if t.action == TradeAction.SELL and t.pnl_sol is not None
    ]
    
    if len(closing_trades) < 5:
        return 0.0  # Need minimum trades for consistency
    
    # Determine wins/losses
    outcomes = [1 if t.pnl_sol > 0 else 0 for t in closing_trades]
    
    # Calculate streak consistency
    # Method 1: Variance of streak lengths
    current_streak = 1
    streaks = []
    
    for i in range(1, len(outcomes)):
        if outcomes[i] == outcomes[i-1]:
            current_streak += 1
        else:
            streaks.append(current_streak)
            current_streak = 1
    streaks.append(current_streak)
    
    if not streaks:
        return 0.0
    
    # Lower variance = more consistent
    avg_streak = sum(streaks) / len(streaks)
    variance = sum((s - avg_streak) ** 2 for s in streaks) / len(streaks)
    
    # Normalize to 0-1 range (inverse relationship)
    # Lower variance = higher consistency
    max_variance = len(outcomes)  # Theoretical maximum
    consistency = 1.0 - min(variance / max_variance, 1.0)
    
    return consistency
```

**Testing:**
- Test with various streak patterns
- Test with all wins (should return high consistency)
- Test with alternating wins/losses
- Verify consistency calculation

---

## Testing Strategy

### Unit Tests

1. **Historical Liquidity Tests:**
   - `test_get_historical_liquidity_exact_match()`
   - `test_get_historical_liquidity_within_tolerance()`
   - `test_get_historical_liquidity_fallback_to_current()`
   - `test_historical_liquidity_interpolation()`

2. **WQS Tests:**
   - `test_wqs_starts_at_zero()` - Verify base score is 0
   - `test_wqs_calculation_pdd_compliant()` - Full PDD compliance test

3. **Metric Calculation Tests:**
   - `test_roi_calculation_accuracy()`
   - `test_win_rate_calculation_accuracy()`
   - `test_drawdown_calculation_accuracy()`
   - `test_win_streak_consistency_calculation()`

### Integration Tests

1. **Backtester with Historical Liquidity:**
   - `test_backtester_uses_historical_liquidity()`
   - `test_backtester_fallback_to_current_liquidity()`
   - `test_backtester_collects_liquidity_snapshots()`

2. **End-to-End Wallet Analysis:**
   - `test_full_wallet_analysis_with_historical_liquidity()`
   - `test_wallet_analysis_with_accurate_metrics()`

### Performance Tests

1. **Historical Liquidity Query Performance:**
   - Test query speed with large historical_liquidity table
   - Test batch insert performance
   - Test cache effectiveness

---

## Migration Strategy

### Database Migration

**No schema changes required** - `historical_liquidity` table already exists.

**Data Migration:**
- Historical liquidity data will be collected incrementally
- No bulk migration needed
- System works with empty historical_liquidity table (falls back to current)

### Code Migration

1. **Phase 1 (Historical Liquidity):**
   - Add new methods to `LiquidityProvider`
   - Update `Backtester` to use historical lookup
   - Deploy incrementally (backward compatible)

2. **Phase 2 (WQS Base Score):**
   - Simple change, deploy with documentation
   - Consider feature flag for gradual rollout

3. **Phase 3 (Enhanced Metrics):**
   - Replace simplified methods with accurate calculations
   - Add feature flag to toggle between old/new methods
   - Monitor for performance impact

### Rollback Plan

1. **Historical Liquidity:**
   - Keep `get_current_liquidity()` method
   - Can revert backtester to use current liquidity
   - No data loss risk

2. **WQS Base Score:**
   - Add config flag: `WQS_BASE_SCORE_MODE = "PDD" | "ENHANCED"`
   - Easy rollback via config

3. **Enhanced Metrics:**
   - Keep old methods as fallback
   - Feature flag for gradual rollout
   - Can revert if performance issues

---

## Success Criteria

### Phase 1: Historical Liquidity ✅

- [ ] `get_historical_liquidity()` method implemented and tested
- [ ] Backtester uses historical liquidity for all trades
- [ ] Fallback to current liquidity works correctly
- [ ] Liquidity snapshots collected during analysis
- [ ] All unit tests pass
- [ ] Integration tests pass

### Phase 2: WQS Base Score ✅

- [ ] WQS calculation starts at 0 (PDD compliant)
- [ ] All existing tests updated and pass
- [ ] Documentation updated
- [ ] No regression in wallet scoring

### Phase 3: Enhanced Metrics ✅

- [ ] ROI calculation is accurate (within 1% of manual calculation)
- [ ] Win rate calculation is accurate
- [ ] Drawdown calculation is accurate
- [ ] Win streak consistency calculation is accurate
- [ ] All tests pass
- [ ] Performance is acceptable (< 2s per wallet analysis)

---

## Risk Assessment

### Low Risk ✅

- **WQS Base Score Change:** Simple change, well-tested
- **Historical Liquidity Fallback:** Backward compatible

### Medium Risk ⚠️

- **Enhanced Metric Calculations:** 
  - May reveal different wallet scores
  - Need to validate against known good wallets
  - Performance impact from price API calls

### Mitigation Strategies

1. **Feature Flags:** Use config flags for gradual rollout
2. **A/B Testing:** Compare old vs new metrics on test wallets
3. **Monitoring:** Track metric calculation performance
4. **Fallback:** Keep old methods available for rollback

---

## Timeline

### Week 1: Historical Liquidity Infrastructure

- **Day 1:** Implement `get_historical_liquidity()` method
- **Day 2:** Update backtester, add collection logic
- **Day 2-3:** Testing and bug fixes

### Week 1: WQS Base Score (Parallel)

- **Day 2:** Update WQS calculation (2-3 hours)
- **Day 2:** Update tests and documentation

### Week 2: Enhanced Metrics

- **Day 3:** Implement accurate ROI calculation
- **Day 4:** Implement win rate and drawdown
- **Day 5:** Implement win streak consistency
- **Day 5:** Testing and validation

### Week 2: Integration & Testing

- **Day 6:** Integration testing
- **Day 7:** Performance testing and optimization
- **Day 8:** Documentation and code review

---

## Dependencies

### External APIs

- **Helius API:** For historical price data (if needed)
- **Birdeye API:** For historical liquidity data (optional)
- **Jupiter API:** For current price/liquidity data

### Internal Dependencies

- **Database:** `historical_liquidity` table (already exists)
- **LiquidityProvider:** Needs enhancement
- **Backtester:** Needs update
- **Analyzer:** Needs metric calculation updates

---

## Documentation Updates

1. **Code Documentation:**
   - Update docstrings for all changed methods
   - Document historical liquidity lookup logic
   - Document metric calculation formulas

2. **User Documentation:**
   - Update Scout module documentation
   - Document historical liquidity collection
   - Update PDD compliance status

3. **API Documentation:**
   - Document new LiquidityProvider methods
   - Update backtester documentation

---

## Conclusion

This plan provides a comprehensive roadmap to fix all identified gaps in the Scout module. The implementation is structured in phases to minimize risk and allow for incremental deployment. All changes are backward compatible with fallback mechanisms.

**Estimated Completion:** 2 weeks  
**Priority:** High (for full PDD compliance)  
**Risk Level:** Low-Medium (well-defined changes with fallbacks)

---

**Plan Created:** 2025-12-06  
**Next Steps:** Review plan, assign tasks, begin Phase 1 implementation




