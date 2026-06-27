# Scout Module User Guide

**Version:** 1.0.0  
**Last Updated:** 2025-12-06

---

## Overview

The Scout module is Chimera's wallet intelligence layer. It analyzes wallet performance from on-chain data, calculates Wallet Quality Scores (WQS), and validates wallets through backtesting before promotion to ACTIVE status.

### Key Features

- ✅ **Historical Liquidity Validation** - Validates trades using liquidity at trade time
- ✅ **Accurate Metric Calculations** - ROI, win rate, drawdown from actual price data
- ✅ **PDD-Compliant WQS Scoring** - Wallet Quality Score starting from 0
- ✅ **Pre-Promotion Backtesting** - Validates wallets before promotion
- ✅ **Automatic Liquidity Collection** - Builds historical liquidity database

---

## Quick Start

### Running Scout

```bash
# Basic run (uses default config)
cd scout
python main.py

# Dry run (analyze without writing)
python main.py --dry-run

# Skip backtest validation (faster)
python main.py --skip-backtest

# Verbose output
python main.py --verbose

# Custom output path
python main.py --output /path/to/roster_new.db
```

### Command Line Options

| Option | Description | Default |
|--------|-------------|---------|
| `--output`, `-o` | Output path for roster_new.db | `../data/roster_new.db` |
| `--dry-run` | Analyze without writing to database | `False` |
| `--skip-backtest` | Skip backtest validation | `False` |
| `--min-wqs-active` | Minimum WQS for ACTIVE status | `70.0` |
| `--min-wqs-candidate` | Minimum WQS for CANDIDATE status | `40.0` |
| `--min-liquidity-shield` | Min liquidity (USD) for Shield | `10000.0` |
| `--min-liquidity-spear` | Min liquidity (USD) for Spear | `5000.0` |
| `--verbose`, `-v` | Enable verbose output | `False` |

---

## How It Works

### 1. Wallet Discovery

Scout discovers wallets from on-chain data:

- Queries Helius API for recent swap transactions
- Extracts wallet addresses from transaction data
- Filters by activity level (minimum trade count)
- Limits to top N wallets (default: 50)

**Configuration:**
```python
analyzer = WalletAnalyzer(
    helius_api_key="your-api-key",
    discover_wallets=True,
    max_wallets=50,
)
```

### 2. Wallet Analysis

For each discovered wallet, Scout:

1. **Fetches Transaction History** - Gets last 30 days of trades
2. **Calculates Metrics:**
   - ROI (7d and 30d) - From actual price changes
   - Win Rate - From actual PnL data
   - Drawdown - Peak-to-trough analysis
   - Win Streak Consistency - Pattern analysis
3. **Calculates WQS** - Wallet Quality Score (0-100)
4. **Collects Liquidity Snapshots** - Stores historical liquidity data

### 3. Pre-Promotion Validation

For wallets with WQS >= 70 (ACTIVE threshold):

1. **Backtest Simulation** - Simulates last 30 days of trades
2. **Historical Liquidity Check** - Uses liquidity at trade time
3. **Slippage & Fee Calculation** - Estimates costs
4. **Validation** - Rejects if simulated PnL < 0

### 4. Roster Output

Scout writes wallet roster to `roster_new.db`:

- **ACTIVE** - WQS >= 70 and passed backtest
- **CANDIDATE** - WQS >= 40 or failed backtest
- **REJECTED** - WQS < 40

The Rust Operator merges this into the main database.

---

## Historical Liquidity

### Overview

Scout now validates trades using **historical liquidity** at the time of each trade, not just current liquidity. This ensures wallets that traded when liquidity was high are properly validated.

### How It Works

1. **During Analysis:** Scout collects liquidity snapshots for each trade
2. **During Backtesting:** Uses historical liquidity from database
3. **Fallback:** If historical unavailable, uses current liquidity

### Database

Historical liquidity is stored in the `historical_liquidity` table:

```sql
CREATE TABLE historical_liquidity (
    token_address TEXT NOT NULL,
    liquidity_usd REAL NOT NULL,
    price_usd REAL,
    volume_24h_usd REAL,
    timestamp TIMESTAMP NOT NULL,
    source TEXT,
    UNIQUE(token_address, timestamp)
);
```

### Query Performance

- Historical liquidity queries: < 100ms
- Tolerance: ±6 hours (configurable)
- Automatic fallback to current liquidity

---

## Enhanced Metrics

### ROI Calculation

**Previous:** Estimated from trade frequency  
**Now:** Calculated from actual price changes

- Tracks entry/exit prices for each position
- Calculates weighted average entry price
- Computes PnL from price differences
- Handles partial position closes

### Win Rate Calculation

**Previous:** Hardcoded 60%  
**Now:** Calculated from actual PnL data

- Only counts SELL trades (closing positions)
- Uses actual PnL from trades
- Counts wins (PnL > 0) vs losses (PnL < 0)

### Drawdown Calculation

**Previous:** Hardcoded 10%  
**Now:** Calculated from running PnL

- Tracks running PnL over time
- Identifies peak values
- Calculates drawdown: (peak - current) / peak
- Returns maximum drawdown percentage

### Win Streak Consistency

**Previous:** Hardcoded 0.5  
**Now:** Calculated from streak patterns

- Analyzes actual win/loss streaks
- Calculates variance of streak lengths
- Factors in win rate
- Returns consistency score (0.0 to 1.0)

---

## WQS Scoring

### PDD Compliance

WQS calculation now starts at **0.0** (PDD compliant), not 50.0.

### Scoring Breakdown

| Component | Points | Description |
|-----------|--------|-------------|
| ROI Performance | Up to 25 | `(roi_30d / 100) * 25` |
| Win Streak Consistency | Up to 20 | `consistency * 20` |
| Anti-Pump-and-Dump | -15 | If `roi_7d > roi_30d * 2` |
| Statistical Significance | 0.5x | If `trade_count < 20` |
| Drawdown Penalty | -0.2x | `drawdown_percent * 0.2` |
| Activity Bonus | +5 | If `trade_count >= 50` |

### Classification

- **ACTIVE:** WQS >= 70.0
- **CANDIDATE:** WQS >= 40.0
- **REJECTED:** WQS < 40.0

---

## Configuration

### Environment Variables

```bash
# Helius API Key
HELIUS_API_KEY=your-api-key

# Database Path
CHIMERA_DB_PATH=/path/to/chimera.db

# RPC URL (API key extracted automatically)
CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=...
```

### Backtest Configuration

```python
from scout.core.models import BacktestConfig

config = BacktestConfig(
    min_liquidity_shield_usd=10000.0,
    min_liquidity_spear_usd=5000.0,
    dex_fee_percent=0.003,  # 0.3%
    max_slippage_percent=0.05,  # 5%
    min_trades_required=5,
)
```

---

## Monitoring

### Logs

Scout logs important events:

- Historical liquidity fallback usage
- Liquidity collection activity
- Backtest validation results
- Wallet promotion/demotion

### Metrics

Monitor these metrics:

- **Historical Liquidity Usage:** % of trades using historical vs current
- **Fallback Rate:** % of trades falling back to current liquidity
- **Backtest Pass Rate:** % of wallets passing backtest
- **Metric Calculation Time:** Time per wallet analysis

---

## Troubleshooting

### Historical Liquidity Not Available

**Symptom:** All trades use current liquidity fallback

**Solution:**
- Ensure `historical_liquidity` table exists
- Check database path is correct
- Run Scout to collect liquidity snapshots
- Wait for historical data to accumulate

### Low Backtest Pass Rate

**Symptom:** Many wallets failing backtest

**Possible Causes:**
- Historical liquidity was lower at trade time
- Slippage/fees eroding profits
- Liquidity thresholds too high

**Solution:**
- Review backtest failure reasons
- Adjust liquidity thresholds if needed
- Check historical liquidity data quality

### Slow Performance

**Symptom:** Scout takes too long to analyze wallets

**Solution:**
- Reduce `max_wallets` limit
- Use `--skip-backtest` for faster analysis
- Check database query performance
- Consider caching frequently accessed data

---

## Best Practices

1. **Run Regularly:** Schedule Scout to run daily via cron
2. **Monitor Fallback Rate:** High fallback rate indicates missing historical data
3. **Review Backtest Failures:** Understand why wallets fail validation
4. **Adjust Thresholds:** Fine-tune WQS and liquidity thresholds based on results
5. **Collect Historical Data:** Let Scout run for several days to build historical liquidity database

---

## API Reference

### WalletAnalyzer

```python
from scout.core.analyzer import WalletAnalyzer

analyzer = WalletAnalyzer(
    helius_api_key="...",
    discover_wallets=True,
    max_wallets=50,
)

# Get candidate wallets
wallets = analyzer.get_candidate_wallets()

# Get wallet metrics
metrics = analyzer.get_wallet_metrics(wallet_address)

# Get historical trades
trades = analyzer.get_historical_trades(wallet_address, days=30)
```

### LiquidityProvider

```python
from scout.core.liquidity import LiquidityProvider

provider = LiquidityProvider(db_path="data/chimera.db")

# Get current liquidity
current = provider.get_current_liquidity(token_address)

# Get historical liquidity
historical = provider.get_historical_liquidity(
    token_address,
    timestamp,
    tolerance_hours=6,
)

# Get historical or fallback to current
liq = provider.get_historical_liquidity_or_current(
    token_address,
    timestamp,
)
```

### BacktestSimulator

```python
from scout.core.backtester import BacktestSimulator
from scout.core.models import BacktestConfig

config = BacktestConfig(...)
simulator = BacktestSimulator(provider, config)

result = simulator.simulate_wallet(
    wallet_address,
    trades,
    strategy="SHIELD",
)
```

---

## Testing

### Run Tests

```bash
cd scout

# Run all tests
pytest tests/ -v

# Run specific test file
pytest tests/test_historical_liquidity.py -v
pytest tests/test_wqs_base_score.py -v
pytest tests/test_enhanced_metrics.py -v
pytest tests/test_backtester_historical_liquidity.py -v
```

### Test Coverage

- ✅ Historical liquidity lookup and storage
- ✅ WQS base score compliance
- ✅ Enhanced metric calculations
- ✅ Backtester with historical liquidity
- ✅ Integration tests

---

## Changelog

### v7.1 (2025-12-06)

- ✅ Added historical liquidity validation
- ✅ Enhanced metric calculations (ROI, win rate, drawdown, consistency)
- ✅ Fixed WQS base score to start at 0 (PDD compliant)
- ✅ Automatic liquidity collection during analysis
- ✅ Batch liquidity storage for efficiency

---

**For more information, see:**
- [Scout Module PDD Review](../docs/scout-module-pdd-review.md)
- [Implementation Plan](../docs/scout-gaps-fix-plan.md)
- [Testing Guide](../scout/TESTING_GUIDE.md)
