# Smart Money Cluster Accumulation Engine Implementation Plan

**Date:** 2026-09-06  
**Status:** APPROVED WITH ARCHITECTURAL AMENDMENTS  
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
2. **Temporal Dispersion Guard:** Total span between first and last wallet must be $\ge 180\text{ seconds}$ (3 minutes), with minimum inter-arrival time $\ge 5\text{ seconds}$ (no two wallets in the same slot/5s window).
3. **True Volume-Weighted Average Price (VWAP):** Drift measurements utilize true mathematical volume weighting: $\sum(P_i \times V_i) / \sum V_i$.
4. **Symmetric Price Drift Envelope:** Token price must reside strictly within $[0.90 \times \text{VWAP},\, 1.15 \times \text{VWAP}]$ to prevent buying runaway pumps ($>+15\%$) or collapsing distribution ($<-10\%$).
5. **Token Position Lock / One-Shot Execution:** Once a cluster triggers an entry for a token, that token is locked against duplicate cluster triggers for 24 hours.
6. **Bilateral Cluster Tracking (Buy & Sell Confluence):** Ingest both `BUY` and `SELL` swaps. If $\ge 2$ cluster wallets dump $>70\%$ of their holdings, trigger an Emergency Cluster Disinvestment Market Sell.
7. **Established Liquidity ($>\$100\text{k}$) & CLMM Slippage Guard:** Enforce $>\$100\text{k}$ pool liquidity AND verify Jupiter quote `priceImpactPct < 1.5%` for a 0.5 SOL order to avoid out-of-range CLMM bins.
8. **Swing Horizon (12h–72h):** Wide disaster stop (−20.0%), tiered profit targets (+35%, +75%, +150%), trailing stop at +40% (trailing 15%), and 72-hour time exit.

---

## 2. Target System Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│               PHASE 1: SCOUT SWING ROSTER SEEDING (Python)             │
│                                                                        │
│  • Discover 50–100 active wallets with >4h avg hold & >55% win rate    │
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
│  • Parameterized single-token aggregation query (indexed lookup)       │
│  • Token Position Lock: Ignore if token has open pos or entered in 24h │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 4: CLUSTER CONFLUENCE TRIGGER (Rust)                 │
│                                                                        │
│  • Query 12-hour window on token: Distinct smart BUY wallets >= 3?     │
│  • Temporal Dispersion: Total span >= 180s, inter-arrival >= 5s        │
│  • Symmetric Drift Guard: 0.90 * VWAP <= current_price <= 1.15 * VWAP  │
│  • Liquidity & CLMM Guard: TVL >= $100k AND priceImpactPct < 1.5%      │
│  • Emit CLUSTER_CONFIRMED (one-shot execution lock)                    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 5: ADMISSION & JITO EXECUTION (Rust)                 │
│                                                                        │
│  • Sizing: 0.25 SOL base ($100k-$250k liq) / 0.50 SOL high-conviction  │
│  • Portfolio Cap: Maximum 4 concurrent open positions (2.0 SOL total)  │
│  • Execution: Jito Bundle via Jupiter with tight dynamic slippage      │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 6: BILATERAL SWING POSITION MONITOR (Rust)           │
│                                                                        │
│  • Disaster Stop: -20.0% hard stop                                     │
│  • Smart Money Disinvestment: If >= 2 cluster wallets dump, sell now   │
│  • Tiered Profit Targets: +35% (sell 33%), +75% (sell 33%), +150% (34%)│
│  • Trailing Stop: Activate at +40%, trail 15%                          │
│  • Stagnation Exit: 72 hours max hold time                             │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Detailed Work Breakdown Structure (WBS)

### Phase 1: Scout Swing Roster Seeding (Python)
**Goal:** Seed 50–100 genuine smart-money swing wallets into the roster. Currently, the database has only 5 ACTIVE wallets, all with `last_trade_at = NULL`.

- [ ] **Task 1.1: Swing Archetype & Liquidity Filters in `scout/core/analyzer.py`**
  - In `determine_archetype`:
    - Require `avg_hold_time >= 14400` (4+ hours) for `TraderArchetype.SWING`.
    - Disqualify wallets trading on tokens with $< \$100\text{k}$ liquidity.
  - In `analyze_wallet`:
    - Require 30-day realized win rate $\ge 55\%$ and profit factor $\ge 1.8$.
- [ ] **Task 1.2: Roster Discovery & Seeding Script `scout/scripts/seed_swing_roster.py`**
  - Query Helius / Dune historical swap activity for top performing swing addresses.
  - Populate `wallets` table with 50–100 validated wallets under status `ACTIVE_SWING`.
  - Update Helius webhook configuration to subscribe to all 50–100 addresses.

---

### Phase 2: Database Schema & Optimized Persistence Layer
**Goal:** Provide durable storage for all smart-money transactions with bilateral support (`BUY` / `SELL`), mathematically sound VWAP, and composite indexes.

- [ ] **Task 2.1: PostgreSQL Migration `0025_smart_money_clusters.sql`**
  ```sql
  CREATE TABLE IF NOT EXISTS smart_money_signals (
      id BIGSERIAL PRIMARY KEY,
      wallet_address TEXT NOT NULL,
      token_address TEXT NOT NULL,
      token_symbol TEXT,
      side TEXT NOT NULL CHECK (side IN ('BUY', 'SELL')),
      amount_sol NUMERIC(30, 18) NOT NULL,
      amount_tokens NUMERIC(38, 18),
      price_usd NUMERIC(30, 18) NOT NULL,
      tx_signature TEXT UNIQUE NOT NULL,
      slot BIGINT,
      block_time TIMESTAMPTZ NOT NULL,
      created_at TIMESTAMPTZ DEFAULT NOW()
  );

  -- Composite index optimized for single-token window queries with INCLUDE clause
  CREATE INDEX IF NOT EXISTS idx_smart_signals_token_window 
  ON smart_money_signals(token_address, side, block_time DESC) 
  INCLUDE (wallet_address, amount_sol, price_usd);

  CREATE INDEX IF NOT EXISTS idx_smart_signals_wallet_time 
  ON smart_money_signals(wallet_address, block_time DESC);

  -- Cluster execution state tracking to prevent duplicate triggers
  CREATE TABLE IF NOT EXISTS cluster_executions (
      token_address TEXT PRIMARY KEY,
      cluster_vwap NUMERIC(30, 18) NOT NULL,
      wallet_count INT NOT NULL,
      entered_at TIMESTAMPTZ DEFAULT NOW(),
      status TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE', 'CLOSED', 'EXPIRED'))
  );
  CREATE INDEX IF NOT EXISTS idx_cluster_executions_entered ON cluster_executions(entered_at DESC);

  -- Global cluster view with true mathematical volume weighting
  CREATE OR REPLACE VIEW v_active_clusters_12h AS
  SELECT 
      token_address,
      COUNT(DISTINCT wallet_address) AS distinct_wallets,
      SUM(amount_sol) AS total_sol_volume,
      MIN(block_time) AS first_signal_at,
      MAX(block_time) AS last_signal_at,
      ROUND((SUM(price_usd * amount_sol) / NULLIF(SUM(amount_sol), 0))::numeric, 8) AS vwap_price_usd,
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
    SELECT 
        COUNT(DISTINCT wallet_address) AS distinct_wallets,
        ROUND((SUM(price_usd * amount_sol) / NULLIF(SUM(amount_sol), 0))::numeric, 8) AS vwap_price_usd,
        EXTRACT(EPOCH FROM (MAX(block_time) - MIN(block_time))) AS cluster_duration_secs,
        ARRAY_AGG(block_time ORDER BY block_time ASC) AS arrival_times
    FROM smart_money_signals
    WHERE token_address = $1 
      AND side = 'BUY'
      AND block_time > NOW() - (INTERVAL '1 hour' * $2)
    GROUP BY token_address;
    ```
  - Implement `is_token_cluster_locked(&self, token_address: &str, cooldown_hours: i64) -> AppResult<bool>`.
  - Implement `record_cluster_execution(&self, token_address: &str, vwap: Decimal, wallet_count: i32) -> AppResult<()>`.
  - Implement `get_cluster_sell_pressure(&self, token_address: &str, since: DateTime<Utc>) -> AppResult<ClusterSellMetrics>`.

---

### Phase 3: Decoupled Ingestion & Concurrency Guards
**Goal:** Persist every raw smart-money swap immediately upon receipt. Prevent race conditions and duplicate orders.

- [ ] **Task 3.1: Ingestion Pipeline in `operator/src/handlers/monitoring.rs`**
  - On incoming swap from Helius:
    1. Extract token, wallet, side (`BUY` or `SELL`), amount, price, slot, signature, timestamp.
    2. Immediately call `db.record_smart_money_signal(...)` asynchronously.
    3. If `side == SELL`:
       - Check if an active Chimera position exists for this token.
       - If yes, pass sell event to `PositionManager::handle_cluster_sell_event()`.
       - Return `Ok(200)`.
    4. If `side == BUY`:
       - Check `db.is_token_cluster_locked(token, 24)` or active in-memory lock. If locked, return `Ok(200)` (ignore duplicate trigger).
       - Query `db.get_token_cluster_metrics(token, 12)`.
       - If `distinct_wallets < 3`: Return `Ok(200)` (holding state).
       - If `distinct_wallets >= 3`:
         - Verify **Temporal Dispersion**:
           - Total span $\ge 180\text{ seconds}$.
           - Inter-arrival times $\ge 5\text{ seconds}$ between consecutive buys.
         - Acquire atomic token lock and route to `signal_pipeline.process_signal()`.

---

### Phase 4: Cluster Admission, Symmetrical Drift, & CLMM Slippage Gates
**Goal:** Ensure entry only occurs within safe price bounds and verified pool liquidity.

- [ ] **Task 4.1: Selection Gates in `operator/src/engine/selection.rs`**
  - **Cluster Confluence Gate:** Strictly require $\ge 3$ distinct wallets.
  - **Symmetric Entry Drift Guard:**
    - Fetch current Jupiter quote for price:
    - If `current_price > 1.15 * cluster_vwap` $\to$ Reject with `CLUSTER_ENTRY_DRIFT_EXCEEDED` (protect against runaway pumps).
    - If `current_price < 0.90 * cluster_vwap` $\to$ Reject with `CLUSTER_ENTRY_PRICE_COLLAPSED` (protect against pool dumps / developer rugs).
  - **Liquidity Floor:** Pool TVL $\ge \$100,000$.
  - **CLMM Slippage / Price Impact Guard:**
    - Query Jupiter Quote API for 0.5 SOL swap.
    - If `quote.price_impact_pct > 1.5%` $\to$ Reject with `CLMM_OUT_OF_RANGE_IMPACT_HIGH` (protect against concentrated liquidity traps).
  - **Token Age Floor:** $\ge 24\text{ hours}$ old.
  - On successful admission, insert record into `cluster_executions` table to lock re-entry for 24 hours.

---

### Phase 5: Position Sizing & Swing Exit Engine
**Goal:** Provide room to breathe through Solana volatility while enforcing bilateral smart-money exit invalidation.

- [ ] **Task 5.1: Sizing in `operator/src/engine/position_sizer.rs`**
  - Base cluster size: `0.25 SOL` (tokens with $\$100\text{k}$–$\$250\text{k}$ liquidity).
  - High-conviction boost: `0.50 SOL` (tokens with $>\$250\text{k}$ liquidity or $\ge 4$ wallets).
  - Portfolio Cap: Maximum 4 concurrent open positions (Max portfolio risk: 2.0 SOL).
- [ ] **Task 5.2: Swing Exit Parameters in `core/src/config.rs` & `operator/src/engine/exit_rules.rs`**
  - `recovery_gate_enabled = false` (eliminate the 15-minute loss cut).
  - `hard_stop_loss_pct = -20.0%` (wide disaster stop).
  - `profit_targets = [35.0, 75.0, 150.0]`.
  - `target_fractions = [0.33, 0.33, 0.34]`.
  - `trailing_stop_activation_pct = +40.0%`, with `15.0%` trailing pullback.
  - `max_hold_time_hours = 72` (stagnation exit).
- [ ] **Task 5.3: Bilateral Smart Money Disinvestment Exit in `operator/src/engine/position_manager.rs`**
  - When `PositionManager::handle_cluster_sell_event(token, wallet)` is called:
    - Track cumulative smart-money liquidations.
    - If $\ge 2$ of the original cluster wallets liquidate $\ge 70\%$ of their tokens:
      - Trigger **Emergency Cluster Disinvestment Market Sell**.
      - Log: `"Cluster invalidation: 2+ smart wallets dumped holdings on {token}. Liquidating position."`

---

## 4. Verification & Testing Plan

### 4.1 Automated Tests
1. **Mathematical VWAP Unit Tests (`infra/tests/vwap_calculation_test.rs`):**
   - Verify volume weighting matches $\sum(P \times V) / \sum V$, rejecting arithmetic means.
2. **Temporal Dispersion Tests (`operator/tests/unit/temporal_dispersion_tests.rs`):**
   - 3 wallets spanning 35 seconds $\to$ Rejected ($<180\text{s}$).
   - 3 wallets spanning 200 seconds with 2 wallets in same slot $\to$ Rejected (inter-arrival $<5\text{s}$).
   - 3 wallets spanning 300 seconds, separated by $>30\text{s}$ each $\to$ Approved.
3. **Drift Guard Bounds Tests (`operator/tests/unit/drift_guard_tests.rs`):**
   - Price at $1.16 \times \text{VWAP}$ $\to$ Rejected (`CLUSTER_ENTRY_DRIFT_EXCEEDED`).
   - Price at $0.88 \times \text{VWAP}$ $\to$ Rejected (`CLUSTER_ENTRY_PRICE_COLLAPSED`).
   - Price at $1.05 \times \text{VWAP}$ $\to$ Approved.
4. **Token Re-Entry Lock Tests (`operator/tests/integration/cluster_lock_tests.rs`):**
   - Wallet 1, 2, 3 buy $\to$ Position opened, token locked.
   - Wallet 4 buys 5 minutes later $\to$ Rejected with `IgnoreAlreadyActive`.
   - Wallet 1 buys again $\to$ Rejected with `IgnoreAlreadyActive`.
5. **Cluster Disinvestment Exit Tests (`operator/tests/integration/cluster_exit_tests.rs`):**
   - Position open at $+5\%$. Wallet A sells 80%, Wallet B sells 90% $\to$ Immediate market sell executed.

### 4.2 Production Server Verification (`chimera-01.moez.tech`)
1. Create and deploy branch `engine/cluster-accumulation`.
2. Apply database migration `0025_smart_money_clusters.sql`.
3. Verify `smart_money_signals` table captures both `BUY` and `SELL` swaps.
4. Verify paper trades trigger only on valid $\ge 3$ wallet clusters with no re-entry duplicates.
5. Track hold durations and monitor `/api/v1/profitability/verdict`.
