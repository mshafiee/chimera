# Scout Parse Rate Improvement Plan

## Current State
- **Parse rate: 0.4%** (CRITICALLY LOW)
- **12 swaps parsed** from **3,094 transactions**
- **2,484 unknown failures** (80.6%)
- **541 no_primary_token failures** (17.6%)
- **57 direction_ambiguous failures** (1.8%)

## Root Cause Analysis

### Primary Issue: Complex Jupiter Routing
Modern Solana trading uses complex multi-hop Jupiter routing that the current parser cannot handle:
- **Multi-hop routing**: 5-11 token transfers per transaction (vs 2-3 expected)
- **Stablecoin bridges**: Multiple USDC/USDT transfers complicating primary token detection
- **Delegated accounts**: Wallets using temporary addresses not in fromUserAccount/toUserAccount
- **Protocol diversity**: New DEX formats not covered by existing parsing strategies

### Sample Failure Analysis
- **Parse fail #1**: 5 tokenTransfers, 4 nativeTransfers, 30 accountData → `no_primary_token`
- **Parse fail #2**: 4 tokenTransfers, 4 nativeTransfers, 22 accountData → `unknown`
- **Parse fail #3**: 8 tokenTransfers, 5 nativeTransfers, 27 accountData → `unknown`

## Solutions

### Immediate (High Impact)

#### 1. Increase Transaction Limit
**Current**: `SCOUT_WALLET_TX_LIMIT=50` (reduced for testing)
**Fix**: Set to default 500 or 1000

**Impact**: More transactions = higher absolute parse success, though rate may stay low

#### 2. Improve Jupiter Routing Detection
Add logic to identify multi-hop routing patterns:

```python
# Detect multi-hop routing
if len(token_transfers) > 5:  # Complex routing
    # Look for common Jupiter routing patterns
    # Filter intermediate stablecoins
    # Identify final destination token
```

**Impact**: Can handle complex routing that current parser misses

#### 3. Enhanced Stablecoin Filtering
Current stablecoin list is good but could be more robust:

```python
stable_mints = {
    usdc_mint,  # USDC
    usdt_mint,  # USDT
    pyusd_mint,  # PYUSD
    "7dHbWXmci3dTUpSFJC3s3nxMPrsrTn5fQjYPb26cscQ",  # USDD
    "DAiHhAwpCe2ygJmhzwQTvXcFqBVRNAGfnUSQ4gNpm5f",  # DAI (legacy)
    "3KBZiQHbjmiNtbaDNqeiyp6Y3qqmANuGphxDjPXqnDVe",  # DAI (official)
    "4MNeZJj3iWc3C7YFU1iXbSsrLuvQEwNyAGTWXfUFguwF",  # TUSD
    "5fTkp16UPQMJyyw7Tm2jrRKTMrMVZh6gHNiYLHNpVV09",  # FDUSD
    "CWGsHHN7LCLfgL8rBFaJMXzyYrRoP7yRgx15fLaTnUuW",  # BUSD
}
```

**Impact**: Better primary token identification in stablecoin-heavy routing

### Medium-term

#### 4. Jupiter API Integration (Strategy 4)
Query Jupiter API for swap details:

```python
async def _parse_swap_from_jupiter_api(tx, wallet_address):
    """Query Jupiter API for swap details as Strategy 4."""
    # Query Jupiter API with transaction signature
    # Parse swap details from API response
    # Fall back to Strategy 3 if API unavailable
```

**Impact**: Direct access to Jupiter swap routing information

#### 5. Transaction Sampling
Capture failed transactions for offline analysis:

```python
if SCOUT_DEBUG_PARSE_FAILURES:
    # Save failed transaction to file
    debug_file = f"data/parse_failures/{sig[:16]}_{reason}.json"
    with open(debug_file, "w") as f:
        json.dump(tx, f, default=str)
```

**Impact**: Enable offline analysis to improve parser

#### 6. Improved Wallet-Owned Account Detection
Enhance algorithm for complex routing:

```python
# Current: Basic SOL flow detection
# Enhanced: Multi-hop pattern recognition
# - Chain of wallet-owned accounts
# - Token flow following SOL flows
# - Stablecoin bridge detection
```

**Impact**: Better handling of delegated wallets

### Long-term

#### 7. Protocol-Specific Parsers
Dedicated parsers for major DEX protocols:
- Jupiter (most important)
- Orca
- Raydium
- PumpFun
- Meteora

**Impact**: Higher success rates for specific protocols

#### 8. Machine Learning Approach
Train model on successfully parsed vs failed transactions:
- Features: token transfer count, account data size, instructions
- Labels: parsed successfully, failure reason
- Model: Random forest or gradient boosting

**Impact**: Adaptive parsing that improves over time

#### 9. Alternative Data Sources
Explore other data sources:
- Jupiter GraphQL API
- DEX-specific APIs
- On-chain direct parsing

**Impact**: Bypass Helius API limitations

## Implementation Priority

1. **Week 1**: Increase transaction limit, improve stablecoin filtering
2. **Week 2**: Jupiter API integration (Strategy 4), transaction sampling
3. **Week 3-4**: Improved wallet-owned account detection, protocol-specific parsers
4. **Month 2-3**: Machine learning approach, alternative data sources

## Success Metrics

- **Target parse rate**: >50% (up from 0.4%)
- **Target swaps parsed**: >1,000 (up from 12)
- **Target reduction in unknown failures**: <500 (down from 2,484)
- **Target reduction in no_primary_token failures**: <100 (down from 541)

## Notes

- This is primarily a **data quality and parsing challenge**, not a rate limiting issue
- Helius API success rate is 90.5%, which is acceptable
- The main issue is that modern Solana trading has outpaced the parsing logic
- Jupiter routing complexity is the primary culprit
- A combination of incremental improvements can significantly improve parse rates
