# Smart Money Cluster Accumulation Engine Implementation Plan

**Date:** 2026-09-06  
**Status:** APPROVED WITH CRITICAL ARCHITECTURAL REVISIONS  
**Branch:** `engine/cluster-accumulation` (Legacy archived at `archive/v1-copy-trading-legacy`)  
**Scope:** Pivot Chimera from reactive 1:1 micro copy-trading to an automated Smart Money Cluster Accumulation Engine.

---

## 1. Executive Summary & Empirical Validation

### 1.1 The Failure of Legacy Micro Copy-Trading
Across 233 realized paper trades and 26,458 shadow positions, Chimera demonstrated a statistically significant negative edge:
* **Mean Net Return:** **−2.10% per trade** (95% CI: `[−3.43%, −0.77%]`).
* **Win Rate:** **20.2%** (All-time), with 71.7% of positions stopped out in $<15$ minutes (median hold: 5.6 minutes).
* **Root Causes:** Adverse selection / front-running on low-liquidity memecoins, 1%–2% DEX fee drag on tiny price moves, and an aggregator deadlock where signals were only recorded *after* admission.

### 1.2 Empirical Validation from Production Database (Last 30 Days)
A historical query across all shadow positions in the production PostgreSQL database (`chimera-01.moez.tech`) over the trailing 30 days definitively proves the cluster hypothesis:

| Strategy | Cluster Size | Sample ($n$) | Win Rate | Average Return | Status |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **`wallet_sell` (Whale exits)** | **1 wallet (Solo)** | 5,295 | 19.2% | **−5.58%** | Heavy Bleed |
| | **2 wallets (Pair)** | 1,158 | 26.6% | **−13.31%** | Worst (Dev + Burner trap) |
| | **3+ wallets (Cluster)** | **4,226** | **37.3%** | **+3.45%** | **Profitable** ✅ |
| **`fixed_24h` (Swing hold)** | **1–2 wallets** | 5,516 | 23.3% | **−14.00%** | Bleed |
| | **3+ wallets (Cluster)** | **4,064** | **47.1%** | **+4.56%** | **Profitable** ✅ |
| **`fixed_4h` (Mid hold)** | **1–2 wallets** | 5,929 | 30.2% | **−11.17%** | Bleed |
| | **3+ wallets (Cluster)** | **4,288** | **46.5%** | **+3.91%** | **Profitable** ✅ |

### 1.3 Core Architectural Axioms
1. **Strict Confluence ($\ge 3$ Distinct Wallets):** 2-wallet pairs perform terribly (−13.31%) because they are frequently dev + burner wallets or wash bots. Cluster trigger requires $\ge 3$ distinct smart wallets.
2. **Distinct Wallet Temporal Dispersion Guard:** Total span between first and last distinct wallet first-buy must be $\ge 180\text{ seconds}$ (3 minutes), with minimum inter-arrival time $\ge 5\text{ seconds}$ between consecutive distinct wallet arrivals (evaluated via `MIN(block_time)` per wallet to prevent split-order false rejections).
3. **Dimensionally Accurate Volume-Weighted Average Price (VWAP):**
   $$\text{VWAP}_{\text{USD}} = \frac{\sum (\text{price\_usd} \times \text{amount\_tokens})}{\sum \text{amount\_tokens}} \equiv \frac{\text{Total USD Spent}}{\text{Total Tokens Acquired}}$$
   $$\text{VWAP}_{\text{SOL}} = \frac{\sum \text{amount\_sol}}{\sum \text{amount\_tokens}}$$
4. **Symmetric Price Drift Envelope (SOL-denominated):** Token price must reside strictly within $[0.90 \times \text{VWAP}_{\text{SOL}},\, 1.15 \times \text{VWAP}_{\text{SOL}}]$ to prevent buying runaway pumps ($>+15\%$) or collapsing distribution ($<-10\%$) while eliminating SOL/USD currency drift over multi-hour accumulation windows.
5. **Transactional Advisory Lock & Cooldown Index:** Wrap cluster admission in PostgreSQL `pg_try_advisory_xact_lock(hashtext(token_address))` to eliminate TOCTOU multi-worker race conditions. Once executed, record in `cluster_executions` (with `id BIGSERIAL PRIMARY KEY` to allow future re-entries after 24h cooldown).
6. **Cumulative Bilateral Disinvestment Exit:** Ingest both `BUY` and `SELL` swaps. Track cumulative tokens sold vs bought per wallet: $\text{Dump Pct}(W) = \sum \text{Sold} / \sum \text{Bought}$. If $\ge 2$ cluster wallets liquidate $\ge 70\%$ of their accumulated holdings, trigger an Emergency Cluster Disinvestment Market Sell.
7. **Established Liquidity ($>\$100\text{k}$) & CLMM Slippage Guard:** Enforce $>\$100\text{k}$ pool liquidity AND verify Jupiter quote `priceImpactPct < 1.5%` for a 0.5 SOL order to avoid out-of-range CLMM bins.
8. **Jito Tip Protection:** Cap Jito tips to $\le 0.0005\text{ SOL}$ for $0.25\text{ SOL}$ sizing (and $\le 0.001\text{ SOL}$ for $0.50\text{ SOL}$) to prevent tip drag from eating profits on non-sniper entries.
9. **Expanded Swing Roster (150–200 Wallets):** Track 150–200 validated swing wallets ($>4\text{h}$ average hold time, $>55\%$ win rate) to ensure healthy cluster candidate velocity ($\sim 2-5$ clusters/week) without sacrificing selection quality.

---

## 2. Target System Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│               PHASE 1: SCOUT SWING ROSTER SEEDING (Python)             │
│                                                                        │
│  • Discover 150–200 active wallets with >4h avg hold & >55% win rate   │
│  • Disqualify 15-minute bonding-curve scalpers                         │
│  • Register addresses in Helius Webhooks (status = 'ACTIVE_SWING')     │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Webhook swaps (BUY / SELL)
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 2 & 3: INGESTION & DECOUPLED PERSISTENCE (Rust)      │
│                                                                        │
│  • Ingest swap at /api/v1/monitoring/helius-webhook                    │
│  • Immediately persist to `smart_money_signals` (side: BUY | SELL)     │
│  • Transactional Advisory Lock: pg_try_advisory_xact_lock(token)       │
│  • Token Cooldown Check: Ignore if token entered within 24h            │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 4: CLUSTER CONFLUENCE TRIGGER (Rust)                 │
│                                                                        │
│  • Query 12-hour window on token: Distinct smart BUY wallets >= 3?     │
│  • Distinct Wallet Arrivals: Span >= 180s, inter-arrival >= 5s         │
│  • Symmetric Drift Guard: 0.90 * VWAP_SOL <= price <= 1.15 * VWAP_SOL  │
│  • Liquidity & CLMM Guard: TVL >= $100k AND priceImpactPct < 1.5%      │
│  • Record into cluster_executions (BIGSERIAL PK)                       │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 5: ADMISSION & JITO EXECUTION (Rust)                 │
│                                                                        │
│  • Sizing: 0.25 SOL base ($100k-$250k liq) / 0.50 SOL high-conviction  │
│  • Jito Tip Cap: <= 0.0005 SOL (0.25 SOL) / <= 0.001 SOL (0.50 SOL)   │
│  • Portfolio Cap: Maximum 4 concurrent open positions (2.0 SOL total)  │
│  • Execution: Jito Bundle via Jupiter with tight dynamic slippage      │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 6: BILATERAL SWING POSITION MONITOR (Rust)           │
│                                                                        │
│  • Disaster Stop: -20.0% hard stop                                     │
│  • Smart Money Disinvestment: If >= 2 cluster wallets dump >= 70% sell │
│  • Tiered Profit Targets: +35% (sell 33%), +75% (sell 33%), +150% (34%)│
│  • Trailing Stop: Activate at +40%, trail 15%                          │
│  • Stagnation Exit: 72h exit if flat/neg; if in profit, breakeven stop │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Detailed Work Breakdown Structure (WBS)

### Phase 1: Scout Swing Roster Seeding (Python)
**Goal:** Seed 150–200 genuine smart-money swing wallets into the roster. Currently, the database has only 5 ACTIVE wallets, all with `last_trade_at = NULL`.

- [ ] **Task 1.1: Swing Archetype & Liquidity Filters in `scout/core/analyzer.py`**
  - In `determine_archetype`:
    - Require `avg_hold_time >= 14400` (4+ hours) for `TraderArchetype.SWING`.
    - Disqualify wallets trading on tokens with $< \$100\text{k}$ liquidity.
  - In `analyze_wallet`:
    - Require 30-day realized win rate $\ge 55\%$ and profit factor $\ge 1.8$.
- [ ] **Task 1.2: Roster Discovery & Seeding Script `scout/scripts/seed_swing_roster.py`**
  - Query Helius / Dune historical swap activity for top performing swing addresses.
  - Populate `wallets` table with 150–200 validated wallets under status `ACTIVE_SWING`.
  - Update Helius webhook configuration to subscribe to all addresses.

---

### Phase 2: Database Schema & Optimized Persistence Layer
**Goal:** Provide durable storage for all smart-money transactions with bilateral support (`BUY` / `SELL`), dimensionally correct VWAP, composite indexes, and reusable cooldown tracking.

- [ ] **Task 2.1: PostgreSQL Migration `infra/migrations_postgres/0023_smart_money_clusters.sql`**
  ```sql
  CREATE TABLE IF NOT EXISTS smart_money_signals (
      id BIGSERIAL PRIMARY KEY,
      wallet_address TEXT NOT NULL,
      token_address TEXT NOT NULL,
      token_symbol TEXT,
      side TEXT NOT NULL CHECK (side IN ('BUY', 'SELL')),
      amount_sol NUMERIC(30, 18) NOT NULL,
      amount_tokens NUMERIC(38, 18) NOT NULL,
      price_usd NUMERIC(30, 18) NOT NULL,
      price_sol NUMERIC(30, 18) NOT NULL,
      tx_signature TEXT NOT NULL,
      slot BIGINT NOT NULL,
      block_time TIMESTAMPTZ NOT NULL,
      created_at TIMESTAMPTZ DEFAULT NOW(),
      CONSTRAINT uq_smart_signal_tx_token_side UNIQUE (tx_signature, token_address, side)
  );

  CREATE INDEX IF NOT EXISTS idx_smart_signals_token_window 
  ON smart_money_signals(token_address, side, block_time DESC) 
  INCLUDE (wallet_address, amount_sol, amount_tokens, price_usd, price_sol);

  CREATE INDEX IF NOT EXISTS idx_smart_signals_wallet_time 
  ON smart_money_signals(wallet_address, block_time DESC);

  CREATE TABLE IF NOT EXISTS cluster_executions (
      id BIGSERIAL PRIMARY KEY,
      token_address TEXT NOT NULL,
      cluster_vwap_usd NUMERIC(30, 18) NOT NULL,
      cluster_vwap_sol NUMERIC(30, 18) NOT NULL,
      wallet_count INT NOT NULL,
      entered_at TIMESTAMPTZ DEFAULT NOW(),
      status TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE', 'CLOSED', 'EXPIRED'))
  );

  CREATE INDEX IF NOT EXISTS idx_cluster_executions_cooldown 
  ON cluster_executions(token_address, entered_at DESC);

  CREATE OR REPLACE VIEW v_active_clusters_12h AS
  SELECT 
      token_address,
      COUNT(DISTINCT wallet_address) AS distinct_wallets,
      SUM(amount_sol) AS total_sol_volume,
      SUM(amount_tokens) AS total_token_volume,
      MIN(block_time) AS first_signal_at,
      MAX(block_time) AS last_signal_at,
      ROUND((SUM(price_usd * amount_tokens) / NULLIF(SUM(amount_tokens), 0))::numeric, 8) AS vwap_price_usd,
      ROUND((SUM(amount_sol) / NULLIF(SUM(amount_tokens), 0))::numeric, 12) AS vwap_price_sol,
      EXTRACT(EPOCH FROM (MAX(block_time) - MIN(block_time))) AS cluster_duration_secs
  FROM smart_money_signals
  WHERE side = 'BUY'
    AND block_time > NOW() - INTERVAL '12 hours'
  GROUP BY token_address
  HAVING COUNT(DISTINCT wallet_address) >= 3;
  ```

- [ ] **Task 2.2: Data Access in `infra/src/db_abstraction/postgres.rs`**
  - Implement `record_smart_money_signal(&self, signal: &SmartMoneySignal) -> AppResult<()>`.
  - Implement single-token query `get_token_cluster_metrics(&self, token_address: &str, window_hours: i64) -> AppResult<Option<TokenClusterMetrics>>`:
    ```sql
    WITH wallet_first_buys AS (
        SELECT 
            wallet_address,
            MIN(block_time) AS first_buy_at,
            SUM(amount_sol) AS wallet_sol,
            SUM(amount_tokens) AS wallet_tokens
        FROM smart_money_signals
        WHERE token_address = $1 
          AND side = 'BUY'
          AND block_time > NOW() - (INTERVAL '1 hour' * $2)
        GROUP BY wallet_address
    )
    SELECT 
        COUNT(DISTINCT wallet_address) AS distinct_wallets,
        ROUND((SUM(wallet_sol) / NULLIF(SUM(wallet_tokens), 0))::numeric, 12) AS vwap_price_sol,
        ARRAY_AGG(first_buy_at ORDER BY first_buy_at ASC) AS distinct_wallet_arrivals
    FROM wallet_first_buys;
    ```
  - Implement `is_token_cluster_locked(&self, token_address: &str, cooldown_hours: i64) -> AppResult<bool>`:
    ```sql
    SELECT EXISTS (
        SELECT 1 FROM cluster_executions
        WHERE token_address = $1
          AND status = 'ACTIVE'
          AND entered_at > NOW() - (INTERVAL '1 hour' * $2)
    );
    ```
  - Implement `record_cluster_execution(&self, token_address: &str, vwap_sol: Decimal, vwap_usd: Decimal, wallet_count: i32) -> AppResult<()>`.
  - Implement `get_cluster_wallet_holdings(&self, token_address: &str, window_hours: i64) -> AppResult<Vec<WalletHoldingSummary>>`:
    ```sql
    SELECT 
        wallet_address,
        COALESCE(SUM(amount_tokens) FILTER (WHERE side = 'BUY'), 0) AS tokens_bought,
        COALESCE(SUM(amount_tokens) FILTER (WHERE side = 'SELL'), 0) AS tokens_sold,
        COALESCE(SUM(amount_tokens) FILTER (WHERE side = 'SELL'), 0)::numeric / 
            NULLIF(SUM(amount_tokens) FILTER (WHERE side = 'BUY'), 0) AS dump_ratio
    FROM smart_money_signals
    WHERE token_address = $1 
      AND block_time > NOW() - (INTERVAL '1 hour' * $2)
    GROUP BY wallet_address;
    ```

---

### Phase 3: Decoupled Ingestion & Advisory Lock Concurrency
**Goal:** Persist every raw smart-money swap immediately upon receipt. Eliminate TOCTOU race conditions.

- [ ] **Task 3.1: Ingestion Pipeline in `operator/src/handlers/monitoring.rs`**
  - On incoming swap from Helius:
    1. Extract token, wallet, side (`BUY` or `SELL`), amount_sol, amount_tokens, price_usd, price_sol, slot, signature, timestamp.
    2. Immediately call `db.record_smart_money_signal(...)` asynchronously.
    3. If `side == SELL`:
       - Check if an active Chimera position exists for this token.
       - If yes, pass sell event to `PositionManager::handle_cluster_sell_event(token, wallet)`.
       - Return `Ok(200)`.
    4. If `side == BUY`:
       - Acquire transactional advisory lock: `SELECT pg_try_advisory_xact_lock(hashtext($1))` on `token_address`. If not acquired, another task is evaluating this token; return `Ok(200)`.
       - Check `db.is_token_cluster_locked(token, 24)`. If locked, return `Ok(200)`.
       - Query `db.get_token_cluster_metrics(token, 12)`.
       - If `distinct_wallets < 3`: Return `Ok(200)` (holding state).
       - If `distinct_wallets >= 3`:
         - Verify **Distinct Wallet Temporal Dispersion**:
           - Total span `arrivals[N-1] - arrivals[0] >= 180 seconds`.
           - Inter-arrival times `arrivals[i] - arrivals[i-1] >= 5 seconds` for all consecutive distinct wallet first buys.
         - Forward cluster payload to `signal_pipeline.process_signal()`.

---

### Phase 4: Cluster Admission, Symmetrical Drift, & CLMM Slippage Gates
**Goal:** Ensure entry only occurs within safe price bounds and verified pool liquidity.

- [ ] **Task 4.1: Selection Gates in `operator/src/engine/selection.rs`**
  - **Cluster Confluence Gate:** Strictly require $\ge 3$ distinct wallets.
  - **Symmetric Entry Drift Guard (SOL Denominated):**
    - Fetch current Jupiter quote for price (Token/SOL):
    - If `current_price_sol > 1.15 * vwap_price_sol` $\to$ Reject with `CLUSTER_ENTRY_DRIFT_EXCEEDED` (protect against runaway pumps).
    - If `current_price_sol < 0.90 * vwap_price_sol` $\to$ Reject with `CLUSTER_ENTRY_PRICE_COLLAPSED` (protect against pool dumps / developer rugs).
  - **Liquidity Floor:** Pool TVL $\ge \$100,000$.
  - **CLMM Slippage / Price Impact Guard:**
    - Query Jupiter Quote API for 0.5 SOL swap.
    - If `quote.price_impact_pct > 1.5%` $\to$ Reject with `CLMM_OUT_OF_RANGE_IMPACT_HIGH` (protect against concentrated liquidity traps).
  - **Token Age Floor:** $\ge 24\text{ hours}$ old.
  - On successful admission, insert record into `cluster_executions` table to lock re-entry for 24 hours.

---

### Phase 5: Position Sizing & Swing Exit Engine
**Goal:** Provide room to breathe through Solana volatility, cap Jito tip drag, and enforce cumulative smart-money exit invalidation.

- [ ] **Task 5.1: Sizing in `operator/src/engine/position_sizer.rs`**
  - Base cluster size: `0.25 SOL` (tokens with $\$100\text{k}$–$\$250\text{k}$ liquidity).
  - High-conviction boost: `0.50 SOL` (tokens with $>\$250\text{k}$ liquidity or $\ge 4$ wallets).
  - Portfolio Cap: Maximum 4 concurrent open positions (Max portfolio risk: 2.0 SOL).
  - **Jito Tip Ceiling:** Cap tip at $\le 0.0005\text{ SOL}$ for $0.25\text{ SOL}$ orders, and $\le 0.001\text{ SOL}$ for $0.50\text{ SOL}$ orders.
- [ ] **Task 5.2: Swing Exit Parameters in `core/src/config.rs` & `operator/src/engine/exit_rules.rs`**
  - `recovery_gate_enabled = false` (eliminate the 15-minute loss cut).
  - `hard_stop_loss_pct = -20.0%` (wide disaster stop).
  - `profit_targets = [35.0, 75.0, 150.0]`.
  - `target_fractions = [0.33, 0.33, 0.34]`.
  - `trailing_stop_activation_pct = +40.0%`, with `15.0%` trailing pullback.
  - `max_hold_time_hours = 72`: If position is in profit, convert to Breakeven stop (+0.5%) with 5% trail; if flat/loss, market sell.
- [ ] **Task 5.3: Cumulative Disinvestment Exit in `operator/src/engine/position_manager.rs`**
  - When `PositionManager::handle_cluster_sell_event(token, wallet)` is called:
    - Query `db.get_cluster_wallet_holdings(token, 12)`.
    - Count wallets where `dump_ratio >= 0.70`.
    - If $\ge 2$ cluster wallets have `dump_ratio >= 0.70`:
      - Trigger **Emergency Cluster Disinvestment Market Sell**.
      - Log: `"Cluster invalidation: 2+ smart wallets dumped >= 70% of accumulated tokens on {token}. Liquidating position."`

---

## 4. Verification & Testing Plan

### 4.1 Automated Tests
1. **Mathematical VWAP Unit Tests (`infra/tests/vwap_calculation_test.rs`):**
   - Verify volume weighting matches $\sum(P \times V) / \sum V$, rejecting arithmetic means and verifying quote/base separation.
2. **Distinct Wallet Temporal Dispersion Tests (`operator/tests/unit/temporal_dispersion_tests.rs`):**
   - 3 wallets spanning 35 seconds $\to$ Rejected ($<180\text{s}$).
   - 3 distinct wallets spanning 200 seconds with 1 wallet having split orders 1s apart $\to$ Approved (split order grouped by wallet).
   - 3 distinct wallets with first buys 2s apart $\to$ Rejected (inter-arrival $<5\text{s}$).
3. **Drift Guard Bounds Tests (`operator/tests/unit/drift_guard_tests.rs`):**
   - Price at $1.16 \times \text{VWAP}$ $\to$ Rejected (`CLUSTER_ENTRY_DRIFT_EXCEEDED`).
   - Price at $0.88 \times \text{VWAP}$ $\to$ Rejected (`CLUSTER_ENTRY_PRICE_COLLAPSED`).
   - Price at $1.05 \times \text{VWAP}$ $\to$ Approved.
4. **Advisory Lock & Re-Entry Tests (`operator/tests/integration/cluster_lock_tests.rs`):**
   - Simultaneous webhooks on 2 threads for same token $\to$ Only 1 evaluates, 2nd skips via `pg_try_advisory_xact_lock`.
   - Token entered $\to$ 2nd buy 1h later rejected with `is_token_cluster_locked`.
   - Token re-entered 25h later $\to$ Allowed (index on `entered_at DESC` with `BIGSERIAL PK`).
5. **Cumulative Disinvestment Exit Tests (`operator/tests/integration/cluster_exit_tests.rs`):**
   - Wallet A bought 1000 tokens, sold 400 then 400 (80% total). Wallet B bought 500, sold 400 (80% total) $\to$ Emergency sell triggered.

### 4.2 Production Server Verification (`chimera-01.moez.tech`)
1. Create and deploy branch `engine/cluster-accumulation`.
2. Apply database migration `infra/migrations_postgres/0023_smart_money_clusters.sql`.
3. Verify `smart_money_signals` table captures both `BUY` and `SELL` swaps.
4. Verify paper trades trigger only on valid $\ge 3$ wallet clusters with no re-entry duplicates.
5. Track hold durations and monitor `/api/v1/profitability/verdict`.
