# Wallet Discovery Architecture

This document describes the wallet discovery system in the Scout module, including
its multi-strategy pipeline, caching layers, configuration options, and resilience
patterns.

## Overview

Wallet discovery is the process of finding active, profitable Solana trader wallets
for analysis. The system uses a **5-strategy fallback chain** with parallel execution,
multi-layer caching, and circuit-breaker protection.

```
                     ┌──────────────────────────┐
                     │  WalletAnalyzer.create() │
                     └────────────┬─────────────┘
                                  │
                     ┌────────────▼─────────────┐
                     │  Multi-Timeframe         │
                     │  Coordinator             │
                     │  (deep / fast / trending)│
                     └────────────┬─────────────┘
                                  │
               ┌──────────────────▼──────────────────┐
               │  HeliusClient                       │
               │  .discover_wallets_from_recent_swaps│
               └──────────────────┬──────────────────┘
                                  │
                    ┌─────────────▼─────────────┐
                    │  Cache Check              │
                    │  (Redis → in-memory)      │
                    └─────────────┬─────────────┘
                       cache miss │
          ┌──────────────────────▼──────────────────────┐
          │  Strategy 1: Active Tokens (sequential)     │
          │  → If wallets < threshold:                  │
          │    Strategy 2-4 in parallel (gather)        │
          │  → If still < max:                          │
          │    Strategy 5: Trending Tokens              │
          └──────────────────────┬──────────────────────┘
                                 │
          ┌──────────────────────▼──────────────────────┐
          │  Validation Pipeline                        │
          │  1. Address format + infrastructure filter  │
          │  2. SOL balance batch-check                 │
          │  3. Activity validation (batch, optional)   │
          │  4. Persistent dedup filter (Redis SET)     │
          │  5. Sort by trade count                     │
          └──────────────────────┬──────────────────────┘
                                 │
          ┌──────────────────────▼──────────────────────┐
          │  Cache + Dedup Write                        │
          │  (Redis + in-memory, mark wallets seen)     │
          └─────────────────────────────────────────────┘
```

## Strategy Chain

### Strategy 1: Active Token Discovery (Primary)

**Method:** `_discover_from_active_tokens()`

Queries recent SWAP transactions for a curated list of high-volume tokens (BONK,
WIF, POPCAT, USDC, SOL, etc.). Uses parallel token queries with a semaphore to
respect rate limits.

- **Config:** `SCOUT_DISCOVERY_LIMIT_PER_TOKEN` (default: 200 transactions/token)
- **Cost:** Low (1 API call per token)

### Strategies 2-4: Parallel Fallback (Secondary)

When Strategy 1 yields fewer wallets than the fallback threshold, three independent
strategies run **concurrently** via `asyncio.gather()`:

| Strategy | Method | Description | Config |
|----------|--------|-------------|--------|
| 2: Recent Blocks | `_discover_from_recent_blocks()` | Scans recent Solana blocks for swap activity | `SCOUT_DISCOVERY_BLOCK_LIMIT` (500) |
| 3: DEX Programs | `_discover_from_dex_programs()` | Queries DEX program accounts (Jupiter, Raydium, Orca) | `SCOUT_DISCOVERY_PROGRAM_LIMIT` (500) |
| 4: Seed Wallets | `_discover_from_seed_wallets()` | Analyzes known profitable wallets' counterparties | `SCOUT_DISCOVERY_SEED_LIMIT` (50) |

Each strategy is wrapped in `_safe_strategy()` which catches exceptions
per-strategy so a single failure doesn't abort the others.

### Strategy 5: Trending Tokens (Tertiary)

**Method:** `discover_from_top_performing_tokens()`

Analyzes currently trending tokens using active-token analysis. Only runs if
strategies 1-4 didn't yield enough wallets.

### Fallback Threshold

The threshold for triggering fallback strategies is configurable:

```
SCOUT_DISCOVERY_FALLBACK_THRESHOLD = 0.5  (50% of max_wallets)
```

If Strategy 1 finds ≥ threshold wallets, strategies 2-4 are skipped entirely.

## Multi-Timeframe Coordinator

The `MultiTimeframeDiscovery` class (`multitimeframe_discovery.py`) orchestrates
discovery across three time horizons:

| Timeframe | Window | Target | Purpose |
|-----------|--------|--------|---------|
| Deep | 720h (30d) | 600 wallets | Established traders with large samples |
| Fast | 24h | 400 wallets | Emerging wallets with recent activity |
| Trending | 4h | 300 wallets | Narrative/hype-driven wallets |

Results are combined, deduplicated, and passed through the profitability
pre-screen before final selection.

## Validation Pipeline

After wallets are discovered, they pass through a multi-stage validation pipeline:

### Stage 1: Address Format + Infrastructure Filter

Validates Solana address format and filters out known system programs, DEX
programs, token mints, and vault addresses.

### Stage 2: SOL Balance Check

Batch-checks SOL balances via RPC `getBalance` calls to filter programs and
vaults that have zero balance.

- **Config:** `SCOUT_VALIDATE_WALLET_BALANCE` (default: true)
- **Fail mode:** `SCOUT_BALANCE_FAIL_MODE` — `open` (include on error) or
  `closed` (exclude on error). Default: `open`.
- **Batch size:** `SCOUT_BALANCE_BATCH_SIZE` (default: 20)

### Stage 3: Activity Validation (Optional)

Validates wallet trading activity (minimum trades, frequency, diversity).

- **Config:** `SCOUT_VALIDATE_WALLET_ACTIVITY` (default: false — expensive)
- **Concurrency:** `SCOUT_ACTIVITY_VALIDATION_CONCURRENCY` (default: 20)
- Uses `_batch_validate_activity()` which runs validations in parallel with
  `asyncio.gather()` + semaphore.

### Stage 4: Persistent Dedup

Filters out wallets that were returned in recent discovery runs using a Redis
SET (`scout:discovery:seen_wallets`) with a configurable TTL.

- **Config:** `SCOUT_DEDUP_TTL` (default: 21600s / 6h)
- Falls back gracefully (no dedup) when Redis is unavailable.

## Caching Layers

### Layer 1: Redis (Persistent)

Discovery results are cached in Redis under key `scout:discovery:{hours_back}:{max_wallets}`
with a configurable TTL. This enables cross-process sharing — multiple Scout
instances can share discovery results.

- **Config:** `SCOUT_DISCOVERY_CACHE_TTL` (default: 3600s / 1h)
- Automatically falls back to in-memory cache when Redis is unavailable.

### Layer 2: In-Memory

Per-process in-memory cache stored on the `HeliusClient` instance. Always updated
even when Redis is available (for same-process cache hits without Redis round-trip).

## Circuit Breaker

Protects against cascading failures by pausing API requests after consecutive
errors.

| Config | Default | Description |
|--------|---------|-------------|
| `SCOUT_CIRCUIT_BREAKER_THRESHOLD` | 5 | Failures before opening |
| `SCOUT_CIRCUIT_BREAKER_RESET_SECONDS` | 60 | Cooldown period |

**State transitions are logged:**
- **Open:** `WARNING [Circuit Breaker] OPENED after N consecutive failures`
- **Reset:** `INFO [Circuit Breaker] Resetting after cooldown`

The `get_rate_limit_stats()` method calls `_check_circuit_breaker()` before
reporting state, preventing stale "open" reports after the cooldown has elapsed.

## API Key Handling

When the Helius API key is missing, discovery behavior depends on the `strict`
parameter:

- **Non-strict** (default): Logs an error with remediation steps and returns `[]`.
  Backward-compatible with existing callers.
- **Strict** (`strict=True`): Raises `DiscoveryError`. Callers must catch it.

```python
# Non-strict (default)
wallets = await client.discover_wallets_from_recent_swaps()

# Strict
try:
    wallets = await client.discover_wallets_from_recent_swaps(strict=True)
except DiscoveryError:
    # Handle missing API key
    pass
```

## Rate Limiting

Uses adaptive rate limiting with:

- Configurable target RPS (`SCOUT_TARGET_RPS`, default: 45)
- Adaptive delay adjustment based on latency and success rate
- Exponential backoff with jitter on 429 responses
- Per-strategy semaphore to limit concurrent API calls
  (`SCOUT_DISCOVERY_CONCURRENCY`, default: 50)

## Configuration Reference

| Environment Variable | Default | Description |
|---------------------|---------|-------------|
| `HELIUS_API_KEY` | — | Helius API key (required for discovery) |
| `SCOUT_DISCOVERY_HOURS` | 168 | Main discovery lookback (7 days) |
| `SCOUT_DISCOVERY_DEEP_HOURS` | 720 | Deep scan lookback (30 days) |
| `SCOUT_DISCOVERY_FAST_HOURS` | 24 | Fast scan lookback |
| `SCOUT_DISCOVERY_TRENDING_HOURS` | 4 | Trending scan lookback |
| `SCOUT_MAX_WALLETS` | 250 | Max wallets to analyze per run |
| `SCOUT_MAX_WALLETS_TIER1` | 150 | Shield candidates (deep) |
| `SCOUT_MAX_WALLETS_TIER2` | 100 | Spear candidates (fast) |
| `SCOUT_DISCOVERY_LIMIT_PER_TOKEN` | 200 | Max transactions per token query |
| `SCOUT_DISCOVERY_BLOCK_LIMIT` | 500 | Max transactions for block scan |
| `SCOUT_DISCOVERY_PROGRAM_LIMIT` | 500 | Max accounts for DEX scan |
| `SCOUT_DISCOVERY_SEED_LIMIT` | 50 | Max transactions per seed wallet |
| `SCOUT_DISCOVERY_FALLBACK_THRESHOLD` | 0.5 | Fraction of max for fallback trigger |
| `SCOUT_DISCOVERY_CONCURRENCY` | 50 | Max concurrent API requests |
| `SCOUT_DISCOVERY_CACHE_TTL` | 3600 | Discovery cache TTL (seconds) |
| `SCOUT_DISCOVERY_PROFITABILITY_FILTER` | true | Pre-screen for profitability |
| `SCOUT_MIN_TRADE_COUNT` | 3 | Minimum trades for inclusion |
| `SCOUT_MIN_SOL_BALANCE` | 0.001 | Minimum SOL balance filter |
| `SCOUT_VALIDATE_WALLET_BALANCE` | true | Enable balance validation |
| `SCOUT_VALIDATE_WALLET_ACTIVITY` | false | Enable activity validation |
| `SCOUT_BALANCE_FAIL_MODE` | open | Balance check fail mode (open/closed) |
| `SCOUT_BALANCE_BATCH_SIZE` | 20 | RPC batch size for balance checks |
| `SCOUT_ACTIVITY_VALIDATION_CONCURRENCY` | 20 | Max concurrent activity validations |
| `SCOUT_DEDUP_TTL` | 21600 | Persistent dedup TTL (6 hours) |
| `SCOUT_MAX_API_CALLS_PER_RUN` | 500 | Max API calls per discovery run |
| `SCOUT_CIRCUIT_BREAKER_THRESHOLD` | 5 | Failures before breaker opens |
| `SCOUT_CIRCUIT_BREAKER_RESET_SECONDS` | 60 | Breaker cooldown |
| `SCOUT_TARGET_RPS` | 45 | Target requests per second |
| `SCOUT_MULTI_TIMEFRAME_DISCOVERY` | true | Enable multi-timeframe mode |
| `REDIS_ENABLED` | true | Enable Redis caching |
| `REDIS_URL` | redis://localhost:6379 | Redis connection URL |

## Key Files

| File | Purpose |
|------|---------|
| `core/helius_client.py` | Primary discovery client, strategies, caching, circuit breaker |
| `core/analyzer.py` | Discovery orchestration, profitability pre-screen, Redis integration |
| `core/multitimeframe_discovery.py` | Multi-timeframe coordinator |
| `core/smart_discovery.py` | Credit-cost-aware strategy prioritization |
| `core/websocket_discovery.py` | Real-time WebSocket-based discovery |
| `core/redis_client.py` | Redis wrapper with in-memory fallback |
| `config.py` | All ScoutConfig settings |
| `config/active_tokens.txt` | Curated token list for Strategy 1 |
| `config/seed_wallets.txt` | Known wallets for Strategy 4 |
| `tests/test_helius_discovery.py` | Unit tests |
| `tests/test_discovery_integration.py` | Integration tests |
