# Performance Baseline

## Measurement Date
YYYY-MM-DD HH:MM:SS UTC

## Environment
- Python version: X.Y.Z
- Rust version: X.Y.Z
- Platform: darwin/linux
- Test mode: fixture replay (zero credit consumption)

## Summary Metrics

### Credits per Wallet
| Wallet | Credits | TX Count |
|--------|---------|----------|
| (TBD after baseline run) | 0 | 0 |

### Latency Statistics
- P50: 0 ms
- P95: 0 ms  
- P99: 0 ms
- Samples: 0

### Memory Usage
- Peak RSS: 0 MB

### Network Calls
- Total calls: 0 (fixture mode)
- Calls per wallet: 0

## Hot Spots Identified
1. **H1** - SWAP→None double-fetch: ~2× credits for edge wallets
2. **H2** - Cache key inconsistency: cache misses across phases
3. **H3** - Pagination cap: up to 2,500 credits/wallet
4. **H4** - Liquidity fetch: per-trade blocking calls

## Optimization Targets
- Reduce credits/wallet by 50%
- Reduce p99 latency by 30%
- Maintain peak RSS ≤ 2 GB

## Next Steps
1. Implement Phase 2 credit fixes
2. Implement Phase 3 latency fix
3. Re-baseline after each fix
4. Verify regression test suites stay green