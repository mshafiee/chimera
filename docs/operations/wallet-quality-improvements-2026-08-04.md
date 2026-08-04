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
