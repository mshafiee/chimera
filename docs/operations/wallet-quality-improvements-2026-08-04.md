# Wallet Quality & Signal Supply Improvements — 2026-08-04

> Complete record of findings, queries, commands, and changes from the wallet quality analysis and signal supply improvement session.

## Table of Contents

1. [Shadow Trade Analysis](#1-shadow-trade-analysis)
2. [Rejection-Rate Wallet Mute Feature](#2-rejection-rate-wallet-mute-feature)
3. [Wallet Pipeline Diagnosis](#3-wallet-pipeline-diagnosis)
4. [Scout Discovery Expansion](#4-scout-discovery-expansion)
5. [Dune Integration for Seed Discovery](#5-dune-integration-for-seed-discovery)
6. [All Production Commands](#6-all-production-commands)
7. [Verification Queries](#7-verification-queries)

---

## 1. Shadow Trade Analysis

### Methodology

Compared shadow paper-trader exits across two lenses:
- **mirror_main** — mirrors the main system's exit logic (looks optimistic)
- **wallet_sell** — follows what the signal wallet actually did (realistic)

### Key Finding: No missed profit opportunities

Every rejection gate is net-negative under the realistic `wallet_sell` measure:

| Gate | wallet_sell net SOL | Verdict |
|------|---------------------|---------|
| NON_SPECULATIVE_TOKEN | −0.001 | Correct (USDC market-maker noise) |
| LIQUIDITY_BELOW_MINIMUM | −0.383 | Correct |
| PUMPFUN_INSUFFICIENT_LIQUIDITY | −0.439 | Correct |
| SIGNAL_QUALITY_TOO_LOW | −1.918 | Correct |
| TOKEN_UNSAFE | −2.964 | Correct |

### Three artifacts that made rejections look like lost profit

1. **Phantom liquidity** — pump.fun tokens show +606% paper gains under `mirror_main` but you physically cannot exit at those prices (rejected for insufficient liquidity). The real wallet exited at −62%, −93%.
2. **Re-entry exploitation** — 106 of 107 "SIGNAL_QUALITY_TOO_LOW" wins were ONE token (`7pjoah1...pump`) re-entered 118×. `fixed_1h` (honest hold) showed −8.5% avg.
3. **pump.fun vs other tokens** — Non-pump.fun rejected tokens: net −0.81 SOL under `mirror_main`. The apparent profit was entirely on untradeable pump.fun tokens.

### Analysis Queries

```sql
-- Per-gate summary by exit strategy
SELECT * FROM shadow_summary_by_gate ORDER BY gate;

-- Lost profit counting distinct tokens once (not re-entries)
WITH best AS (
  SELECT sc.token_address, sc.main_rejection_code AS gate,
         max(sc.pnl_pct) AS best_pnl_pct, max(sc.pnl_sol) AS best_pnl_sol
  FROM shadow_comparison sc
  WHERE sc.exit_strategy='mirror_main' AND sc.main_admitted=false AND sc.pnl_sol > 0
  GROUP BY sc.token_address, sc.main_rejection_code
)
SELECT gate, count(DISTINCT token_address) AS distinct_tokens,
       avg(best_pnl_pct) AS avg_best_pnl, sum(best_pnl_sol) AS sum_best_pnl_sol
FROM best GROUP BY gate ORDER BY sum_best_pnl_sol DESC;

-- Realistic wallet_sell net by gate
SELECT main_rejection_code, count(*), avg(pnl_pct), sum(pnl_sol)
FROM shadow_comparison
WHERE main_admitted=false AND exit_strategy='wallet_sell'
GROUP BY main_rejection_code ORDER BY sum(pnl_sol) DESC;
```

---

## 2. Rejection-Rate Wallet Mute Feature

### Problem

Wallets that are NEVER admitted (all signals rejected) are invisible to existing demotion mechanisms:
- `ToxicFlowDetector` fires on ROI drop of admitted trades → never-admitted wallets have 0 admitted trades → never flagged
- `WalletPerformanceTracker` same issue
- Result: `7wXtGay` (USDC market-maker) generated 3,931 rejected signals and was never flagged

### Solution

New `RejectionMuteDetector` (mirrors `ToxicFlowDetector` pattern):

| Component | File | Description |
|-----------|------|-------------|
| Migration | `migrations_postgres/0016_rejection_mute.sql` | `muted_wallets` table |
| Config | `config.rs` `RejectionMuteConfig` | 90% threshold, 50-signal window, 6h mute |
| Detector | `engine/rejection_mute.rs` | Rolling-window per-wallet hard-rejection tracking |
| Gate | `selection.rs` `decide_buy()` | `WALLET_MUTED` rejection after toxic gate |
| Recording | `selection.rs` `decide()` | Records BUY outcomes after decision finalized |
| Wiring | `main.rs` | Startup load, 5-min periodic persist, shutdown persist |

### Hard vs Soft Rejection Classification

```
Hard (counts toward mute):              Soft (does NOT count):
  NON_SPECULATIVE_TOKEN                   LIQUIDITY_BELOW_MINIMUM
  TOKEN_UNSAFE                            SIGNAL_QUALITY_TOO_LOW
  PUMPFUN_INSUFFICIENT_LIQUIDITY          WQS_TOO_LOW
  PUMPFUN_BONDING_CURVE                   TOKEN_TOO_NEW / TOKEN_AGE_UNKNOWN
  INVALID_TOKEN_ADDRESS                   PORTFOLIO_HEAT_LIMIT / STRATEGY_HEAT_LIMIT
  TOKEN_FAST_CHECK_ERRORED                POSITION_SIZE_ZERO / POSITION_SIZER_ERROR
                                          TOXIC_WALLET / WALLET_MUTED / WALLET_NOT_ACTIVE
```

### Config (config.yaml)

```yaml
rejection_mute:
  enabled: true
  window_size: 50           # track last 50 BUY decisions per wallet
  min_window_samples: 20    # need >=20 decisions before muting
  hard_rejection_threshold: 0.90  # mute at >=90% hard-rejection rate
  mute_duration_hours: 6    # mute for 6h, then re-evaluate
```

### Commits

```
6091a8a feat(rejection_mute): add muted_wallets migration (0016)
fa22a3f feat(rejection_mute): add RejectionMuteConfig to AppConfig
16fdb46 feat(rejection_mute): add RejectionMuteDetector with rolling-window muting
45fcbac feat(rejection_mute): integrate WALLET_MUTED gate and decision recording
eae5435 feat(rejection_mute): wire detector into main.rs
d4e4f92 feat(rejection_mute): add config.yaml section with production defaults
```

### Live Verification

- `7wXtGay` muted 57 seconds after operator restart (100% hard-rejection rate, 20-signal window)
- NON_SPECULATIVE_TOKEN rejections dropped from ~3,931/2h to **0** in 2 min
- Periodic persistence working (every 5 min → `muted_wallets` table)

---

## 3. Wallet Pipeline Diagnosis

### Wallet Counts by Status

| Status | Count | Avg WQS |
|--------|-------|---------|
| CANDIDATE | 11,235 | 22.8 |
| REJECTED | 1,282 | 2.0 |
| ACTIVE | 28 | 53.8 |

### WQS Distribution

| WQS Tier | ACTIVE | CANDIDATE |
|----------|--------|-----------|
| 80+ (SHIELD-tier) | 5 | 0 |
| 60-80 | 7 | 0 |
| 40-60 | 7 | 3 |
| 20-40 | 9 | 13 |
| <20 | 0 | 11,219 |

### ACTIVE Wallet Productivity (24h)

| Category | Count | Description |
|----------|-------|-------------|
| Productive (admitted trades) | 4 | `7oLD`, `Grxr6`, `A2AhB`, `oFz62` |
| Signals but all rejected | 4 | pump.fun/unsafe token traders |
| Dormant (0 signals, WQS 52-105) | 7 | `A4LH8`, `BUDDEw`, `J5ne5`, etc. |
| No signals at all | 13 | inactive |

### Root Causes

1. **7 dormant high-WQS wallets** — WQS 52-105, webhooks healthy, but NEVER produced a speculative signal. Dead weight occupying ACTIVE slots.
2. **`max_active_wallets: 30`** too tight — only 2 slots remained.
3. **Scout discovery pump.fun-dominated** — 0 new candidates in 24h; wallets get WQS instant-rejected (0% admission rate → circular problem).
4. **WQS confidence adjustment crushes borderline wallets** — raw WQS 30+ but adjusted to 0 due to confidence < 0.10.

### Fix Applied

`max_active_wallets` raised from 30 → 50 (commit `68e6269`).

---

## 4. Scout Discovery Expansion

### Issues Found & Fixed

#### Bug: Raydium pattern using wrong address

```python
# BEFORE (bug): using mSOL token mint instead of Raydium AMM program
"mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So": "raydium",

# AFTER (fixed): correct Raydium AMM v4 program ID
"675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8": "raydium",
```

#### Meteora DLMM added to DEX programs

```python
# config.py get_dex_program_ids()
"LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",  # Meteora DLMM

# helius_client.py dex_program_patterns
"LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo": "meteora",
```

#### Fake placeholder tokens removed

18 fake addresses removed from `active_tokens.txt` (TOKEN, ASSET, COIN, SWAP, TRADE, etc. — all with pattern `XxAbCdEfGhIjKlMnOpQrStUvWxYz1234567890`). These were wasting discovery API calls.

#### Token seed list cleaned

Reduced from 101 lines (many fake/obscure) to ~20 curated high-liquidity tokens on established DEXes.

#### Seed wallets added

- **100 top traders** from external CSV ranking (`Top_100_Solana_Traders_Last_Day.csv`) added to `seed_wallets.txt`
- **71 Dune-discovered profitable traders** (Meteora/Raydium/Orca, ROI > 1.1) added to `seed_wallets.txt`
- **Top 30 from each source** (60 total) inserted as CANDIDATE wallets for direct WQS evaluation

### Commits

```
05d72b3 feat(scout): add Meteora DLMM, fix Raydium bug, clean token list
49a9fc4 feat(scout): add top 100 Solana traders as seed wallets
da87d1b feat(scout): add 71 Dune-discovered profitable Solana DEX traders as seeds
```

### Files Modified

| File | Change |
|------|--------|
| `scout/config.py` | Added Meteora DLMM to `get_dex_program_ids()` |
| `scout/core/helius_client.py` | Fixed Raydium pattern, added Meteora to `dex_program_patterns` |
| `scout/config/active_tokens.txt` | Removed 18 fake tokens, curated to ~20 established tokens |
| `scout/config/seed_wallets.txt` | Added 171 seed wallets (100 CSV + 71 Dune) |

---

## 5. Dune Integration for Seed Discovery

### Dune Query

**Query ID**: 8221520 (private)  
**URL**: [dune.com/queries/8221520](https://dune.com/queries/8221520)

```sql
WITH trades AS (
    SELECT
        trader_id AS wallet,
        amount_usd,
        token_bought_symbol,
        token_sold_symbol,
        CASE
            WHEN token_sold_symbol IN ('SOL', 'WSOL', 'USDC', 'USDT') THEN amount_usd
            ELSE 0
        END AS sell_usd,
        CASE
            WHEN token_bought_symbol IN ('SOL', 'WSOL', 'USDC', 'USDT') THEN amount_usd
            ELSE 0
        END AS buy_usd
    FROM dex_solana.trades
    WHERE block_time > NOW() - INTERVAL '7' DAY
      AND project IN ('meteora', 'raydium', 'whirlpool')
      AND amount_usd > 50
      AND NOT (token_bought_mint_address LIKE '%pump' OR token_sold_mint_address LIKE '%pump')
      AND trader_id IS NOT NULL
)
SELECT
    wallet,
    COUNT(*) AS trade_count,
    ROUND(SUM(amount_usd), 0) AS total_volume_usd,
    ROUND(SUM(sell_usd), 0) AS sell_volume_usd,
    ROUND(SUM(buy_usd), 0) AS buy_volume_usd,
    ROUND(SUM(sell_usd) - SUM(buy_usd), 0) AS net_pnl_usd,
    ROUND(SUM(sell_usd) / NULLIF(SUM(buy_usd), 0), 2) AS roi,
    COUNT(DISTINCT COALESCE(NULLIF(token_bought_symbol, ''), token_sold_symbol)) AS unique_tokens
FROM trades
GROUP BY wallet
HAVING COUNT(*) >= 5
  AND SUM(amount_usd) > 1000
  AND SUM(buy_usd) > 0
ORDER BY net_pnl_usd DESC
LIMIT 200
```

### Dune Table Reference

- **`dex_solana.trades`** — Dune's decoded Solana DEX trade table (31 columns)
- Key columns: `trader_id` (wallet address, varchar), `project` (DEX name), `amount_usd`, `token_bought_mint_address`/`token_sold_mint_address` (varchar)
- Solana project names: `pumpswap`, `pumpdotfun`, `meteora`, `raydium`, `whirlpool`, `jupiterz`, `pancakeswap`
- **Note**: `dex.trades` is EVM-only (no Solana). Use `dex_solana.trades` for Solana.

### Dune API — Create Query

```bash
curl -X POST "https://api.dune.com/api/v1/query" \
  -H "X-Dune-Api-Key: $DUNE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Query Name",
    "query_sql": "SELECT ...",
    "is_private": true
  }'
# Returns: {"query_id": 8221520}
```

### Dune API — Execute & Get Results

```bash
# Execute
curl -X POST "https://api.dune.com/api/v1/query/8221520/execute" \
  -H "X-Dune-Api-Key: $DUNE_API_KEY"
# Returns: {"execution_id": "01KZ..."}

# Poll for completion (state: QUERY_STATE_PENDING → EXECUTING → COMPLETED)
curl "https://api.dune.com/api/v1/execution/$EXEC_ID/results" \
  -H "X-Dune-Api-Key: $DUNE_API_KEY"

# Download CSV
curl "https://api.dune.com/api/v1/query/8221520/results/csv" \
  -H "X-Dune-Api-Key: $DUNE_API_KEY" -o traders.csv
```

### Dune API — Update Query

```bash
curl -X PATCH "https://api.dune.com/api/v1/query/8221520" \
  -H "X-Dune-Api-Key: $DUNE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query_sql": "SELECT ..."}'
```

### Weekly Refresh Script

```bash
#!/bin/bash
# Refresh seed wallets from Dune weekly
DUNE_API_KEY="${DUNE_API_KEY:?Set DUNE_API_KEY env var}"

# Download latest results
curl -s "https://api.dune.com/api/v1/query/8221520/results/csv" \
  -H "X-Dune-Api-Key: $DUNE_API_KEY" -o /tmp/dune_traders.csv

# Process: filter copy-tradable wallets, append to seed_wallets.txt
python3 -c "
import csv
with open('/tmp/dune_traders.csv') as f:
    for r in csv.DictReader(f):
        try:
            trades = int(r['trade_count'])
            roi = float(r['roi'])
            pnl = float(r['net_pnl_usd'])
            tokens = int(r['unique_tokens'])
        except (ValueError, KeyError):
            continue
        if trades <= 2000 and tokens <= 50 and roi >= 1.1 and pnl > 100:
            print(f\"{r['wallet']}  # dune ROI={roi:.2f} pnl=\${pnl:.0f} trades={trades}\")
" >> scout/config/seed_wallets.txt

echo 'Run: git add scout/config/seed_wallets.txt && git commit && git push'
echo 'Then: ssh root@chimera-01.moez.tech \"cd /opt/chimera && git pull && docker compose ... up -d --force-recreate scout\"'
```

### Results Summary

- 200 profitable traders found on Meteora/Raydium/Orca
- 71 filtered as copy-tradable (not market makers, ROI > 1.1, < 50 tokens)
- Top performer: `9HsFJKqo` — ROI 2.34, $1M net PnL, 23 trades

---

## 6. All Production Commands

### Deploy Operator

```bash
# Local: commit and push
git add -A && git commit -m "description" && git push origin main

# Server: pull and rebuild
ssh root@chimera-01.moez.tech
cd /opt/chimera
git pull origin main
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build operator
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator
```

### Deploy Scout

```bash
ssh root@chimera-01.moez.tech
cd /opt/chimera
git pull origin main
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml build scout
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate scout
```

### Config-Only Change (no rebuild needed)

```bash
# Just restart the container to pick up config.yaml changes
ssh root@chimera-01.moez.tech "cd /opt/chimera && git pull origin main && \
  COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator"
```

### Insert Wallets as Candidates (direct SQL)

```bash
ssh root@chimera-01.moez.tech "docker exec -i chimera-postgres psql -U chimera -d chimera" << 'EOF'
INSERT INTO wallets (address, status) VALUES
  ('WALLET_ADDRESS_HERE', 'CANDIDATE')
ON CONFLICT (address) DO UPDATE SET status = CASE
  WHEN wallets.status = 'ACTIVE' THEN 'ACTIVE' ELSE 'CANDIDATE' END;
EOF
```

### Build & Test Commands

```bash
make build              # Build all
make build-operator     # Rust operator (release)
make test-operator      # Rust tests (--test-threads=1 for integration)
make test-scout         # Python pytest
make lint-operator      # cargo clippy -- -D warnings
make lint-scout         # ruff check .
make lint-web           # ESLint
```

---

## 7. Verification Queries

### Check Muted Wallets

```sql
SELECT left(wallet_address,12) AS wallet, is_muted, muted_until, window_size
FROM muted_wallets WHERE is_muted ORDER BY muted_until;
```

### Check Rejection Codes Over Time

```sql
SELECT rejection_code, count(*)
FROM decision_records
WHERE decided_at > NOW() - INTERVAL '10 minutes'
GROUP BY rejection_code ORDER BY count DESC;
```

### Check Wallet Productivity

```sql
SELECT left(w.address,12) AS wallet, round(w.wqs_score::numeric,1) AS wqs,
       count(dr.decision_id) AS decisions_24h,
       sum(CASE WHEN dr.admitted THEN 1 ELSE 0 END) AS admitted_24h
FROM wallets w
LEFT JOIN decision_records dr ON dr.wallet_address = w.address
    AND dr.decided_at > NOW() - INTERVAL '24 hours'
WHERE w.status = 'ACTIVE'
GROUP BY w.address, w.wqs_score
ORDER BY admitted_24h DESC NULLS LAST, decisions_24h DESC NULLS LAST;
```

### Check Scout Discovery Activity

```bash
# Scout logs
docker exec chimera-scout grep -E 'completed|WQS.*FINAL|INSTANT-REJECT' /app/data/logs/scout.log | tail -20

# New candidates
docker exec chimera-postgres psql -U chimera -d chimera -c \
  "SELECT count(*) FROM wallets WHERE created_at > NOW() - INTERVAL '2 hours';"
```

### Check Operator Logs

```bash
# Logs go to file, not stdout
docker exec chimera-operator tail -20 /app/data/logs/operator.log.2026-08-04

# Grep for specific events
docker exec chimera-operator grep -i 'muted\|RejectionMute\|WALLET_MUTED' /app/data/logs/operator.log.2026-08-04 | tail -10
```

### Check Wallet Counts

```sql
SELECT status, count(*), round(avg(wqs_score)::numeric,1) AS avg_wqs
FROM wallets GROUP BY status ORDER BY count DESC;
```

---

## Appendix: Architecture After Changes

```
Signal Intake Pipeline (with rejection mute):

  Webhook/Helius signal
    │
    ▼
  SelectionService::decide()
    ├── decide_buy()
    │     ├── Wallet status gate (WALLET_NOT_ACTIVE)
    │     ├── Toxic wallet gate (TOXIC_WALLET)           ← existing
    │     ├── Rejection mute gate (WALLET_MUTED)         ← NEW
    │     ├── WQS gate (WQS_TOO_LOW)
    │     ├── Token safety (TOKEN_UNSAFE)
    │     ├── Liquidity (LIQUIDITY_BELOW_MINIMUM)
    │     ├── Signal quality (SIGNAL_QUALITY_TOO_LOW)
    │     └── Heat limits
    │
    ├── record_decision() → RejectionMuteDetector        ← NEW
    │     ├── Rolling window (last 50 BUY decisions)
    │     ├── Hard rejection rate ≥ 90% → mute 6h
    │     └── Time-boxed: auto-unmute after expiry
    │
    └── Shadow trader (forks every signal)

Scout Discovery Pipeline (expanded):

  Token seeds (active_tokens.txt)
    │
    ├── Strategy 1: Active token SWAP transactions
    ├── Strategy 2: Recent blocks
    ├── Strategy 3: DEX program accounts
    │     ├── Jupiter, Raydium AMM v4, Orca, Whirlpool
    │     └── Meteora DLMM                              ← NEW
    ├── Strategy 4: Seed wallet counterparties
    │     ├── 100 external CSV traders                  ← NEW
    │     └── 71 Dune-discovered traders                ← NEW
    └── Strategy 5: Trending tokens

  → WQS scoring → CANDIDATE → auto_promote → ACTIVE (cap: 50)
```

---

## 8. Dune Profitability Strategies

Six strategies for leveraging Dune Analytics to improve system profitability, ranked by priority.

### Priority Matrix

| Priority | Strategy | Effort | Impact | Dune Query ID |
|----------|----------|--------|--------|---------------|
| 1 | Wallet PnL scoring (fast demotion) | Low | High | TBD |
| 2 | Token pre-screening cache | Medium | High | TBD |
| 3 | MEV bot exclusion | Low | Medium | TBD |
| 4 | Entry timing analysis | Medium | High | TBD |
| 5 | Token holder concentration | Medium | Medium | TBD |
| 6 | Consensus smart money clusters | High | Medium | TBD |

---

### Strategy 1: Real-Time Wallet PnL Scoring (Fast Demotion Loop)

**Problem:** `WalletPerformanceTracker` only fires after 4+ admitted losing trades. By then money is already lost. WQS recomputes every 2h via expensive Helius RPC calls.

**Solution:** Compute every ACTIVE wallet's 24h rolling PnL cheaply from `dex_solana.trades`. If PnL turns negative, demote immediately — before the next admitted trade loses money.

**Integration:** Run hourly via Dune API. Any ACTIVE wallet with negative 24h PnL gets auto-demoted. This catches failing wallets 2-4 hours before the current performance tracker would.

```sql
-- Active wallet 24h PnL monitor (run hourly)
WITH wallet_pnl AS (
    SELECT
        trader_id AS wallet,
        COUNT(*) AS trades_24h,
        ROUND(SUM(CASE WHEN token_sold_symbol IN ('SOL','WSOL','USDC','USDT')
            THEN amount_usd ELSE 0 END) -
        SUM(CASE WHEN token_bought_symbol IN ('SOL','WSOL','USDC','USDT')
            THEN amount_usd ELSE 0 END), 0) AS net_pnl_usd,
        ROUND(SUM(amount_usd), 0) AS volume_usd
    FROM dex_solana.trades
    WHERE block_time > NOW() - INTERVAL '24' HOUR
      AND project IN ('meteora', 'raydium', 'whirlpool', 'pumpswap')
      AND amount_usd > 10
    GROUP BY trader_id
    HAVING SUM(amount_usd) > 100
)
SELECT wallet, trades_24h, net_pnl_usd, volume_usd,
       ROUND(net_pnl_usd * 100.0 / NULLIF(volume_usd, 0), 2) AS margin_pct
FROM wallet_pnl
WHERE net_pnl_usd < -50  -- losing more than $50 in 24h
ORDER BY net_pnl_usd ASC
LIMIT 100
```

**Operator integration:** Poll Dune API hourly → demote any ACTIVE wallet appearing in results.

---

### Strategy 2: Token Pre-Screening Gate (Kill Rugs Before Entry)

**Problem:** `TOKEN_UNSAFE` and `PUMPFUN_INSUFFICIENT_LIQUIDITY` gates reject ~87% of signals, but they still consume RPC calls, liquidity checks, and latency before rejection.

**Solution:** Build a daily-refreshed token quality cache from Dune. Before even checking token safety via RPC, look up the token in the cache.

```sql
-- Token quality screen (run daily, cache top 5000 tokens)
SELECT
    COALESCE(token_bought_mint_address, token_sold_mint_address) AS token_mint,
    MAX(COALESCE(token_bought_symbol, token_sold_symbol)) AS symbol,
    COUNT(*) AS swap_count_24h,
    ROUND(SUM(amount_usd), 0) AS volume_usd_24h,
    COUNT(DISTINCT trader_id) AS unique_traders,
    MAX(block_time) AS last_trade,
    ROUND(SUM(amount_usd) / NULLIF(COUNT(*), 0), 0) AS avg_trade_size,
    SUM(CASE WHEN project IN ('meteora','raydium','whirlpool') THEN 1 ELSE 0 END) AS dex_venue_trades,
    SUM(CASE WHEN project = 'pumpdotfun' THEN 1 ELSE 0 END) AS bonding_curve_trades
FROM dex_solana.trades
WHERE block_time > NOW() - INTERVAL '24' HOUR
  AND amount_usd > 1
GROUP BY 1
HAVING SUM(amount_usd) > 500
ORDER BY volume_usd_24h DESC
LIMIT 5000
```

**Operator integration:** Cache top 5000 tokens daily in a `token_quality_cache` table. In `selection.rs`, add a fast pre-gate: if token not in cache OR `dex_venue_trades = 0`, reject instantly as `TOKEN_NO_DEX_LIQUIDITY` — no RPC call needed.

---

### Strategy 3: MEV Bot & Wash Trader Exclusion

**Problem:** MEV bots and wash traders generate signals that look profitable but are uncopyable. Their trades are sandwich attacks or self-trades, not directional bets.

**Solution:** Detect and blacklist them weekly.

```sql
-- Detect MEV/sandwich bots
-- Pattern: trades within 1 slot of large swaps (front-running)
WITH large_swaps AS (
    SELECT block_slot, trader_id, amount_usd, token_bought_mint_address
    FROM dex_solana.trades
    WHERE block_time > NOW() - INTERVAL '24' HOUR
      AND amount_usd > 5000
),
sandwich_bots AS (
    SELECT s.trader_id, COUNT(*) AS sandwich_count
    FROM dex_solana.trades s
    JOIN large_swaps l
      ON s.block_slot BETWEEN l.block_slot - 1 AND l.block_slot + 1
     AND s.trader_id != l.trader_id
     AND s.amount_usd > 100
    WHERE s.block_time > NOW() - INTERVAL '24' HOUR
    GROUP BY s.trader_id
    HAVING COUNT(*) > 20
)
SELECT trader_id AS wallet, sandwich_count
FROM sandwich_bots
ORDER BY sandwich_count DESC
```

**Operator integration:** Run weekly. Add detected wallets to a `dune_blacklist` table. The rejection-mute detector or a new pre-gate checks this list before processing signals.

---

### Strategy 4: Profitable Entry Timing Analysis (Personalized Exits)

**Problem:** Fixed exit strategies (1h, 4h, 24h) don't match each wallet's actual trading rhythm. `7oLD` hits profit targets in ~8 minutes on bounces, but bleeds over 1h.

**Solution:** Analyze each tracked wallet's historical entry-to-exit timing to set personalized exit strategies.

```sql
-- Per-wallet optimal hold time
-- Pairs buys and sells of the same token by the same wallet
WITH buys AS (
    SELECT trader_id, token_bought_mint_address AS token,
           block_time AS buy_time, amount_usd AS buy_usd
    FROM dex_solana.trades
    WHERE block_time > NOW() - INTERVAL '7' DAY
      AND token_bought_symbol IN ('SOL','WSOL','USDC','USDT')
      AND project IN ('meteora','raydium','whirlpool')
),
sells AS (
    SELECT trader_id, token_sold_mint_address AS token,
           block_time AS sell_time, amount_usd AS sell_usd
    FROM dex_solana.trades
    WHERE block_time > NOW() - INTERVAL '7' DAY
      AND token_sold_symbol IN ('SOL','WSOL','USDC','USDT')
      AND project IN ('meteora','raydium','whirlpool')
)
SELECT
    b.trader_id AS wallet,
    ROUND(AVG(DATE_DIFF('minute', b.buy_time, s.sell_time)), 0) AS avg_hold_min,
    ROUND(AVG(s.sell_usd - b.buy_usd), 0) AS avg_pnl_usd,
    ROUND(AVG(CASE WHEN s.sell_usd > b.buy_usd THEN 1.0 ELSE 0.0 END) * 100, 1) AS win_rate_pct,
    COUNT(*) AS round_trips
FROM buys b
JOIN sells s ON b.trader_id = s.trader_id
    AND b.token = s.token
    AND s.sell_time > b.buy_time
    AND DATE_DIFF('hour', b.buy_time, s.sell_time) < 48
GROUP BY b.trader_id
HAVING COUNT(*) >= 5
ORDER BY avg_pnl_usd DESC
LIMIT 100
```

**Operator integration:** Use `avg_hold_min` per wallet to set personalized exit strategies instead of fixed 1h/4h/24h. A wallet that profits in 15 minutes gets a 20-minute exit; a swing trader gets 4h.

---

### Strategy 5: Token Holder Concentration (Rug Risk Screening)

**Problem:** Can't easily check if top holders control most of supply — this requires expensive per-token RPC calls.

**Solution:** Batch-compute holder concentration from Dune trade volumes as a proxy.

```sql
-- Token holder concentration (rug risk indicator)
WITH holder_volume AS (
    SELECT
        COALESCE(token_bought_mint_address, token_sold_mint_address) AS token,
        trader_id,
        SUM(CASE WHEN token_bought_amount > 0 THEN amount_usd ELSE 0 END) AS buy_usd
    FROM dex_solana.trades
    WHERE block_time > NOW() - INTERVAL '7' DAY
      AND project IN ('meteora','raydium','whirlpool')
      AND amount_usd > 10
    GROUP BY 1, 2
),
ranked AS (
    SELECT token, trader_id, buy_usd,
           ROW_NUMBER() OVER (PARTITION BY token ORDER BY buy_usd DESC) AS holder_rank,
           SUM(buy_usd) OVER (PARTITION BY token) AS total_buy_usd
    FROM holder_volume
)
SELECT
    token,
    ROUND(SUM(CASE WHEN holder_rank <= 3 THEN buy_usd ELSE 0 END) * 100.0
          / NULLIF(MAX(total_buy_usd), 0), 1) AS top3_concentration_pct,
    COUNT(DISTINCT trader_id) AS total_holders,
    MAX(total_buy_usd) AS total_volume
FROM ranked
GROUP BY token
HAVING MAX(total_buy_usd) > 1000
ORDER BY top3_concentration_pct DESC
LIMIT 500
```

**Operator integration:** If `top3_concentration_pct > 70%`, flag token as `HIGH_HOLDER_CONCENTRATION` — reject or reduce position size. Top 3 holders dumping = guaranteed rug.

---

### Strategy 6: Consensus Smart Money (Wallet Clusters)

**Problem:** Individual wallet signals are noisy. But when 3+ profitable wallets buy the same token within minutes, that's high conviction.

**Solution:** Find wallets that consistently co-trade profitably and weight their consensus signals higher.

```sql
-- Find co-trading profitable wallet clusters
-- Wallets that buy the same token within 10 minutes of each other
WITH timed_buys AS (
    SELECT
        trader_id AS wallet,
        token_bought_mint_address AS token,
        block_time,
        amount_usd,
        DATE_TRUNC('minute', block_time) AS buy_minute
    FROM dex_solana.trades
    WHERE block_time > NOW() - INTERVAL '24' HOUR
      AND token_bought_symbol IN ('SOL','WSOL','USDC','USDT')
      AND project IN ('meteora','raydium','whirlpool')
      AND amount_usd > 100
),
co_buys AS (
    SELECT
        a.token,
        a.wallet AS wallet_a,
        b.wallet AS wallet_b,
        ABS(DATE_DIFF('second', a.block_time, b.block_time)) AS time_delta_sec
    FROM timed_buys a
    JOIN timed_buys b ON a.token = b.token
      AND a.wallet < b.wallet
      AND a.buy_minute = b.buy_minute
)
SELECT wallet_a, wallet_b, COUNT(*) AS co_trade_count
FROM co_buys
WHERE time_delta_sec < 600  -- within 10 minutes
GROUP BY wallet_a, wallet_b
HAVING COUNT(*) >= 3  -- co-traded 3+ times in 24h
ORDER BY co_trade_count DESC
LIMIT 50
```

**Operator integration:** When 2+ wallets from the same cluster signal on the same token, boost the signal's quality score. This turns existing consensus detection into a profitability-weighted consensus.
