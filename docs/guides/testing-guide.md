# Testing Guide for New Features

This guide covers testing procedures for the newly implemented features.

## 1. Consensus Detection Testing

### Unit Tests
Run consensus detection unit tests:
```bash
cd operator
cargo test consensus_detection_tests --lib
```

### Integration Test
Test with real database:
```bash
cargo test test_consensus_detection_two_wallets --test integration_tests
```

### Manual Testing
1. Send webhook signals from 2+ different wallets for the same token within 5 minutes
2. Check logs for: `"Consensus signal detected"`
3. Verify signal quality score is higher (consensus adds 30% weight)
4. Check that consensus signals get wider stop-losses

### Expected Behavior
- First wallet signal: No consensus, normal quality score
- Second wallet signal (same token, within 5 min): Consensus detected, higher quality score
- Signals >5 minutes apart: No consensus
- SELL signals: Never trigger consensus

## 2. Helius Token Age Integration

### Integration Test (Requires API Key)
```bash
# Set API key
export HELIUS_API_KEY="your-api-key"

# Run tests
cargo test --test helius_token_age_tests -- --ignored
```

### Manual Testing
1. Send a webhook signal for a token
2. Check logs for: `"Failed to fetch token age from Helius"` or token age value
3. Verify token age is used in signal quality calculation (10% weight)
4. Test with known tokens (USDC, USDT) - should return age > 24 hours
5. Test with new tokens - should return None or small age

### Expected Behavior
- Known tokens: Return age in hours (e.g., USDC ~3 years = ~26,000 hours)
- New tokens: Return None or small age (< 1 hour)
- API failures: Gracefully handle, use None (doesn't break signal quality)

## 3. Volatility Calculation Testing

### Unit Tests
```bash
cargo test volatility_tests --test integration_tests
```

### Manual Testing
1. Monitor SOL price updates in logs
2. After 24h of price data, check volatility calculation
3. Test market condition filtering:
   - High volatility (>30%): Trades should be rejected
   - SOL crash (>10% in 1h): Trades should be rejected
   - Normal volatility: Trades should proceed

### Expected Behavior
- Insufficient data (< 2 price points): Return None
- Normal market: Volatility 5-15%
- High volatility: >30% (trades blocked)
- Price crash: >10% drop in 1h (trades blocked)

### Monitoring in Production
```bash
# Check price cache stats
curl http://localhost:8080/api/v1/metrics

# Monitor volatility in logs
tail -f logs/operator.log | grep volatility
```

## 4. Multi-DEX Comparison Testing

### Integration Test (Requires Network)
```bash
cargo test --test dex_comparison_tests -- --ignored
```

### Manual Testing
1. Execute a trade
2. Check logs for DEX selection: `"Selected DEX: Jupiter"` or other DEX
3. Verify all DEXs are queried in parallel (check timing)
4. Verify lowest cost DEX is selected
5. Test caching: Same token pair within 5 seconds should use cache

### Expected Behavior
- Queries Jupiter, Raydium, Orca, Meteora in parallel
- Selects DEX with lowest total cost (fee + slippage)
- Caches results for 5 seconds
- Falls back to Jupiter if all queries fail

### API Endpoints to Monitor
- Jupiter: `https://quote-api.jup.ag/v6/quote`
- Raydium: `https://api.raydium.io/v2/swap/quote`
- Orca: `https://api.orca.so/v1/quote`
- Meteora: `https://dlmm-api.meteora.ag/pair/quote`

## 5. Auto-Demotion Testing

### Configuration
Auto-demotion is now enabled in `config/config.yaml`:
```yaml
monitoring:
  auto_demote_wallets: true
```

### Manual Testing
1. Create a test wallet with poor performance
2. Simulate 7+ days of copy trading with losses
3. Verify `should_demote()` returns true
4. Check that wallet status changes: `ACTIVE` → `CANDIDATE`
5. Verify reason is logged: `"Auto-demoted: Copy PnL < 70% of expected"`

### Expected Behavior
- Wallet with poor copy performance (< 70% of expected ROI for 7+ days)
- Status automatically changes to `CANDIDATE`
- Reason logged in database
- Wallet no longer receives signals

### Disable Auto-Demotion
If needed, disable in config:
```yaml
monitoring:
  auto_demote_wallets: false
```

## Running All Tests

### Unit Tests Only
```bash
cd operator
cargo test --lib
```

### Integration Tests (No Network)
```bash
cargo test --test integration_tests
```

### Integration Tests (With Network - Requires API Keys)
```bash
export HELIUS_API_KEY="your-key"
cargo test --test integration_tests -- --ignored
```

### Specific Feature Tests
```bash
# Consensus detection
cargo test consensus_detection

# Volatility
cargo test volatility

# DEX comparison
cargo test dex_comparison

# Helius integration
cargo test helius_token_age
```

## Production Monitoring

### Key Metrics to Monitor
1. **Consensus Detection Rate**: % of signals that are consensus
2. **Token Age Fetch Success**: % of successful Helius API calls
3. **Volatility Rejections**: Number of trades rejected due to volatility
4. **DEX Selection Distribution**: Which DEX is selected most often
5. **Auto-Demotion Events**: Number of wallets auto-demoted

### Log Patterns to Watch
```bash
# Consensus signals
grep "Consensus signal detected" logs/operator.log

# Volatility rejections
grep "High market volatility detected" logs/operator.log

# DEX selection
grep "Selected DEX" logs/operator.log

# Auto-demotion
grep "Auto-demoting wallet" logs/operator.log
```

## Troubleshooting

### Consensus Not Detected
- Check SignalAggregator is initialized in webhook handler
- Verify signals are within 5-minute window
- Check database for signal_aggregation table entries

### Helius API Failures
- Verify API key is set correctly
- Check rate limits (Helius has rate limits)
- Token age is optional - failures don't break signal quality

### Volatility Always None
- Ensure SOL is tracked: `price_cache.track_token("So11111111111111111111111111111111111111112")`
- Wait for price cache updater to populate history
- Check price cache updater is running

### DEX Comparison Always Jupiter
- Check network connectivity
- Verify API endpoints are accessible
- Check logs for API errors
- May be expected if other DEXs are unavailable

### Auto-Demotion Not Working
- Verify `auto_demote_wallets: true` in config
- Check wallet has 7+ days of history
- Verify copy PnL < 70% of expected ROI
- Check database for status updates




