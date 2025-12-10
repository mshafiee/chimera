# Testing Guide: Verifying Scout Finds Most Profitable Wallets

This guide explains how to test the Python Scout code to ensure it correctly identifies and ranks the most profitable wallets.

## Quick Test

Run the comprehensive test suite:

```bash
cd scout
python test_profitable_wallets.py
```

This will:
1. Test WQS ranking with 8 different wallet scenarios
2. Verify profitability detection
3. Test full analyzer integration
4. Test edge cases

## Test Scenarios

### 1. Unit Tests (WQS Calculation)

Test the core WQS calculation logic:

```bash
cd scout
python -m pytest tests/test_wqs.py -v
python -m pytest tests/test_wqs_properties.py -v
```

**What it tests:**
- WQS bounds (always 0-100)
- ROI contribution
- Win rate/consistency scoring
- Pump-and-dump detection
- Statistical significance penalties
- Drawdown penalties
- Activity bonuses

### 2. Integration Test (Full Pipeline)

Test the complete wallet analysis pipeline:

```bash
cd scout
python main.py --dry-run --verbose
```

**What it tests:**
- Wallet analyzer with sample data
- WQS calculation for multiple wallets
- Backtest validation (if enabled)
- Wallet classification (ACTIVE/CANDIDATE/REJECTED)
- Ranking by profitability

**Expected output:**
- Top wallets should have highest WQS scores
- Profitable wallets (high ROI, good win rate) should rank highest
- Pump-and-dump wallets should be penalized
- Low trade count wallets should be penalized

### 3. Comprehensive Test Suite

Run the custom profitability test:

```bash
cd scout
python test_profitable_wallets.py
```

**What it tests:**
- 8 different wallet scenarios with known characteristics
- Verifies correct ranking (most profitable = highest WQS)
- Tests edge cases (pump patterns, low trade count, high drawdown)
- Validates profitability detection

## Test Wallet Scenarios

The test suite includes these wallet types:

1. **Highly Profitable** - Should rank #1
   - ROI 30d: 65%
   - Trade count: 150
   - Win rate: 75%
   - Low drawdown: 5%
   - High consistency: 0.80

2. **Consistent Profitable** - Should rank #2
   - ROI 30d: 45%
   - Trade count: 120
   - Win rate: 70%
   - Moderate drawdown: 8%
   - Very high consistency: 0.75

3. **Moderate Profitable** - Should rank #3
   - ROI 30d: 28%
   - Trade count: 80
   - Win rate: 65%
   - Higher drawdown: 12%

4. **Pump and Dump** - Should rank LOW
   - ROI 7d: 200% (spike!)
   - ROI 30d: 25% (much lower)
   - Should be penalized -15 points

5. **Low Trade Count** - Should rank LOW
   - ROI 30d: 50% (good)
   - Trade count: 8 (very low)
   - Should get 0.25x multiplier penalty

6. **High Drawdown** - Should rank LOW
   - ROI 30d: 40% (good)
   - Drawdown: 35% (very high)
   - Should lose ~7 points (35 * 0.2)

7. **Losing Wallet** - Should rank LOWEST
   - ROI 30d: -20% (negative)
   - Win rate: 35%
   - Should be REJECTED

8. **Break Even** - Should rank LOW
   - ROI 30d: 2% (minimal)
   - Win rate: 50%
   - Should be CANDIDATE or REJECTED

## Verifying Correct Ranking

The test verifies:

1. **Most profitable wallets rank highest**
   - High ROI + High trade count + Good win rate = High WQS

2. **Pump patterns are penalized**
   - 7d ROI > 2x 30d ROI triggers -15 point penalty

3. **Low trade count is penalized**
   - < 10 trades: 0.25x multiplier
   - < 20 trades: 0.5x multiplier

4. **High drawdown is penalized**
   - Each 1% drawdown = -0.2 points

5. **Consistency is rewarded**
   - Win streak consistency contributes up to 20 points

6. **Activity is rewarded**
   - 50+ trades get +5 point bonus

## Testing with Real Data

To test with real wallet data:

1. **Modify WalletAnalyzer** to fetch from real APIs:
   ```python
   analyzer = WalletAnalyzer(
       helius_api_key="your-key",
       rpc_url="https://mainnet.helius-rpc.com/?api-key=..."
   )
   ```

2. **Add real wallet addresses** to candidate list:
   ```python
   analyzer._candidate_wallets = [
       "real_wallet_address_1",
       "real_wallet_address_2",
       # ... more addresses
   ]
   ```

3. **Run analysis**:
   ```bash
   python main.py --verbose --output test_roster.db
   ```

4. **Verify results**:
   - Check that wallets with highest ROI rank highest
   - Verify pump patterns are detected
   - Confirm low trade count wallets are penalized

## Expected Test Results

When running `test_profitable_wallets.py`, you should see:

```
✓ Highly profitable wallet ranks #1
✓ Consistent profitable wallet ranks #2
✓ Pump and dump wallet correctly penalized
✓ Low trade count wallet correctly penalized
✓ Losing wallet correctly ranks very low
✓ All profitable wallets correctly classified as ACTIVE or CANDIDATE
```

## Key Metrics for Profitability

The WQS system considers:

1. **ROI Performance** (up to 25 points)
   - 30-day ROI is primary indicator
   - Capped at 100% for scoring

2. **Consistency** (up to 20 points)
   - Win streak consistency preferred
   - Win rate as fallback

3. **Statistical Significance**
   - < 10 trades: 0.25x multiplier (harsh penalty)
   - < 20 trades: 0.5x multiplier
   - 50+ trades: +5 point bonus

4. **Risk Management**
   - Drawdown penalty: -0.2 points per 1% drawdown
   - High drawdown = poor risk management

5. **Pump Detection**
   - 7d ROI > 2x 30d ROI: -15 point penalty
   - Prevents lucky spike wallets from ranking high

## Troubleshooting

If tests fail:

1. **Check WQS calculation**:
   ```python
   from core.wqs import calculate_wqs, WalletMetrics
   metrics = WalletMetrics(roi_30d=50.0, trade_count_30d=100, ...)
   wqs = calculate_wqs(metrics)
   print(f"WQS: {wqs}")
   ```

2. **Verify ranking logic**:
   - Higher ROI should generally = higher WQS
   - But penalties can affect ranking (pump, low trades, drawdown)

3. **Check edge cases**:
   - Negative ROI wallets should score low
   - Very high ROI with low trades should be penalized
   - Pump patterns should be detected

## Continuous Testing

For CI/CD, add to your test suite:

```bash
# Run all tests
pytest tests/ -v

# Run profitability test
python test_profitable_wallets.py

# Run dry-run analysis
python main.py --dry-run --verbose
```

## Success Criteria

The Scout correctly finds profitable wallets if:

1. ✅ Wallets with highest ROI rank highest (when other factors equal)
2. ✅ Consistent profitable wallets rank above volatile ones
3. ✅ Pump-and-dump patterns are detected and penalized
4. ✅ Low trade count wallets are penalized (statistical significance)
5. ✅ High drawdown wallets are penalized (risk management)
6. ✅ Losing wallets are rejected or rank very low
7. ✅ Top-ranked wallets are classified as ACTIVE (WQS >= 70)
