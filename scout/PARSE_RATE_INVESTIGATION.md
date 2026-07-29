# Scout Parse Rate Investigation Summary

## Current State
- **Parse rate: 0.4%** (CRITICALLY LOW)
- **12 swaps parsed** from **3,094 transactions** (down from 2.2% in previous run)
- **Failure breakdown:**
  - `unknown`: 2,484 (80.6%) - All three parsing strategies failing
  - `no_primary_token`: 541 (17.6%) - Cannot identify which token to track
  - `direction_ambiguous`: 57 (1.8%) - Cannot determine buy/sell direction

## Root Cause: Complex Jupiter Routing

Modern Solana trading uses complex multi-hop Jupiter routing that the current parser cannot handle:

- **Multi-hop routing**: 5-11 token transfers per transaction (vs 2-3 expected)
- **Stablecoin bridges**: Multiple USDC/USDT transfers complicating primary token detection
- **Delegated accounts**: Wallets using temporary addresses not in fromUserAccount/toUserAccount
- **Protocol diversity**: New DEX formats not covered by existing parsing strategies

## Transaction Complexity Examples

- **Parse fail #1**: 5 tokenTransfers, 4 nativeTransfers, 30 accountData → `no_primary_token`
- **Parse fail #2**: 4 tokenTransfers, 4 nativeTransfers, 22 accountData → `unknown`
- **Parse fail #3**: 8 tokenTransfers, 5 nativeTransfers, 27 accountData → `unknown`

## What Was Fixed

1. **Syntax error in analyzer.py**: Changed `continue` to `return None` in `_parse_swap_to_trade` (line 1443)
2. **Created comprehensive improvement plan**: `scout/improve_parse_rate.md`

## What Was Investigated

- Helius API success rate: 90.5% (not the issue)
- Transaction structure: Complex multi-hop routing (root cause)
- Parsing logic: Three-tier strategy insufficient for modern trading
- Sample failures: Detailed analysis of transaction complexity

## Next Steps Recommendations

### Immediate (Week 1):

1. **Increase transaction limit**: Change `SCOUT_WALLET_TX_LIMIT=50` → `500` in docker-compose.yml
2. **Improve stablecoin filtering**: Expand stablecoin list and improve filtering logic
3. **Add transaction sampling**: Enable debug mode to capture failed transactions

### Medium-term (Week 2-4):

1. **Jupiter API integration**: Add Strategy 4 to query Jupiter API directly
2. **Improved wallet-owned account detection**: Enhance algorithm for complex routing
3. **Protocol-specific parsers**: Dedicated parsers for major DEX protocols

### Long-term (Month 2-3):

1. **Machine learning approach**: Train model on parsed vs failed transactions
2. **Alternative data sources**: Jupiter GraphQL API, DEX-specific APIs

## Success Metrics

- **Target parse rate**: >50% (up from 0.4%)
- **Target swaps parsed**: >1,000 (up from 12)
- **Target reduction in unknown failures**: <500 (down from 2,484)

## Key Finding

This is primarily a **data quality and parsing challenge**, not a rate limiting issue. The main issue is that modern Solana trading has outpaced the parsing logic. Jupiter routing complexity is the primary culprit, and a combination of incremental improvements can significantly improve parse rates.

## Additional Notes

- The current 0.4% parse rate is critically low and severely limits Scout's ability to identify profitable wallets
- The three-tier parsing strategy (deltas, events, accountData) is insufficient for modern Solana trading
- Complex Jupiter routing with multiple intermediate tokens and stablecoins is the primary cause
- Recommended to start with immediate fixes (transaction limit, stablecoin filtering) before more complex solutions
- Full implementation plan available in `scout/improve_parse_rate.md`