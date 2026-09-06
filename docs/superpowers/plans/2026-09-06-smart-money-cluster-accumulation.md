# Smart Money Cluster Accumulation Engine Implementation Plan

**Date:** 2026-09-06  
**Status:** ⛔ HELD FOR EMPIRICAL RE-VALIDATION (was: APPROVED)  
**Branch:** `engine/cluster-accumulation` (Legacy archived at `archive/v1-copy-trading-legacy`)  
**Scope:** Pivot Chimera from reactive 1:1 micro copy-trading to an automated Smart Money Cluster Accumulation Engine.

---

> ## ⛔ HOLD NOTICE — 2026-09-06 Production Audit
>
> The empirical foundation in §1.2 **does not reproduce against the production database** and may not be used to justify Phase 1:
>
> 1. **The cluster dimension was never recorded.** `shadow_positions.consensus_wallet_count` contains only `1` (205 rows) or `NULL` (26,317 rows). `decision_records`: 178,307 NULL / 2,430× count=1 / **1× count=2**. Zero ≥3-wallet clusters have ever been observed by the pipeline.
> 2. **Root cause (aggregator deadlock, confirmed):** `SignalAggregator::add_signal` (`infra/src/monitoring/signal_aggregator.rs:105`) only counts signals from the tracked roster. The roster currently has **5 ACTIVE wallets** (`SELECT status, COUNT(*) FROM wallets` → 5 ACTIVE / 57 PROVING), making multi-wallet consensus unobservable. §1.1 already names this defect; §1.2 presents statistics it could not have produced.
> 3. **No generating artifact exists.** No script/query in the repo produces §1.2's table. Provenance unknown — treat those numbers as unvalidated until the exact query is recovered and reviewed for lookahead/survivorship bias.
> 4. **Honest 12h token-window self-join reconstruction INVERTS the result** (trailing 30d): wallet_sell solo −2.02% (n=8,424), pair −6.31% (n=1,489), 3+ cluster **−5.32%** (n=761); fixed_24h 3+ **−8.03%** (n=729) vs. claimed +4.56%. The only profitable historical signal is the retired `dune_wallet` strategy (+52% avg, ended 2026-08-07) — wallet selection alpha, not a cluster property.
>
> **Re-validation sequence before any Phase 1 work:**
> - **Gate 0:** Recover/locate the §1.2 query; audit for lookahead & survivorship bias. Fix signal ingestion so every tracked-wallet BUY is recorded (`smart_money_signals`-style, pre-admission) and `consensus_wallet_count` is actually populated. Expand the roster so consensus is testable.
> - **Gate 1:** Re-test the cluster hypothesis on repaired data with a pre-registered definition (window, liquidity floor, archetype filters). If cluster EV is confirmed negative, pivot strategy (e.g., high-conviction solo swingers) — do not build the engine.
> - **Gate 2:** Only then apply the reviewed engine fixes (stale `EVALUATING` lease takeover, status-aware cooldown, atomic portfolio cap in the reservation INSERT, canonical UI token units, `mpsc`-decoupled webhook ingestion, `CloseAccount` on terminal exits) and proceed to Phase 1.
>
> **Gate 0 outcome (2026-09-06):** §1.2 query unrecoverable (no artifact in repo). Ingestion fix shipped: migration `0023`+`0024` (`smart_money_signals`, pre-admission, idempotent, live in prod), durable 12h consensus attribution in `selection.rs` step 7. See `docs/superpowers/plans/2026-09-06-gate0-gate1-signal-recording-revalidation.md`.
>
> **Gate 1 outcome (2026-09-06): PIVOT.** Pre-registered rule (n≥300, avg>0, bootstrap 95% CI-lo>0) rejected every 3+ cluster bucket across wallet_sell/fixed_24h/fixed_4h — all negative: −5.37% / −7.86% / −3.28%. Pairs are the worst bucket (−6.5% to −12.7%); solo is also negative. Edge is in wallet selection, not cluster size. Full results: `docs/superpowers/analysis/2026-09-06-cluster-revalidation-results.md`. **Engine remains HELD — pivot review required.**

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
2. **Subset-Valid Temporal Dispersion Guard:** At least one valid subset of $\ge 3$ distinct smart wallets must satisfy both total span $\ge 180\text{ seconds}$ and consecutive inter-arrival $\ge 5\text{ seconds}$. Subsequent entries within $<5\text{s}$ do not invalidate an existing valid cluster.
3. **Dimensionally Accurate Volume-Weighted Average Price (VWAP):**
   $$\text{VWAP}_{\text{SOL}} = \frac{\sum \text{amount\_sol}_i}{\sum \text{amount\_tokens}_i}, \quad \text{VWAP}_{\text{USD}} = \frac{\sum (\text{price\_usd}_i \times \text{amount\_tokens}_i)}{\sum \text{amount\_tokens}_i}$$
   Intermediate precision is retained fully in PostgreSQL `numeric` and mapped to `rust_decimal::Decimal`.
4. **Symmetric Price Drift Envelope (SOL-denominated):** Token price must reside strictly within $[0.90 \times \text{VWAP}_{\text{SOL}},\, 1.15 \times \text{VWAP}_{\text{SOL}}]$ to prevent buying runaway pumps ($>+15\%$) or collapsing distribution ($<-10\%$) while eliminating SOL/USD currency drift over multi-hour accumulation windows.
5. **Atomic Reservation Pattern (Zero TOCTOU, No Open Trans Locks):**
   Uses an atomic reservation row in `cluster_executions` with a partial unique index:
   ```sql
   CREATE UNIQUE INDEX idx_cluster_active_lock ON cluster_executions(token_address) 
   WHERE status IN ('EVALUATING', 'ACTIVE');
   ```
   This prevents concurrent Tokio tasks from evaluating or buying the same token simultaneously, without holding open database transactions across async HTTP calls.
6. **Anchored Bilateral Disinvestment Exit:** Stores `cluster_wallets TEXT[]` in `cluster_executions`. Evaluates cumulative holdings starting from $(T_{\text{entered}} - 12\text{h})$ up to `NOW()`. If $\ge 2$ cluster constituent wallets dump $\ge 70\%$ of their accumulated holdings, triggers an immediate market sell.
7. **24-Hour Cooldown Independent of Trade Status:** Cooldown evaluates `entered_at > NOW() - INTERVAL '24 hours'` regardless of whether the position is `ACTIVE` or `CLOSED`.
8. **Fee-Optimized Sizing & Take-Profits:**
   * For base sizes ($\le 0.35\text{ SOL}$), consolidate to 2 exit tiers: 50% at $+35\%$, 50% at $+75\%$ (with $+40\%$ activation / $15\%$ trailing stop) to avoid fixed fee drag.
   * Jito tip hard cap: $\le 0.0005\text{ SOL}$ for $0.25\text{ SOL}$ sizing.
9. **Verified Liquidity & CLMM Slippage:** Pool TVL $\ge \$100,000$ (verified via DexScreener client in `TokenMetadataClient`) AND Jupiter quote `priceImpactPct < 1.5%` for a 0.5 SOL order.
10. **Expanded Swing Roster (150–200 Wallets):** Track 150–200 validated swing wallets ($>4\text{h}$ average hold time, $>55\%$ win rate) to ensure healthy cluster candidate velocity without lowering confluence requirements.

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
│             PHASE 2 & 3: INGESTION & ATOMIC RESERVATION (Rust)         │
│                                                                        │
│  • Ingest swap at /api/v1/monitoring/helius-webhook                    │
│  • Immediately persist to `smart_money_signals` (side: BUY | SELL)     │
│  • 24h Cooldown check: Any entry in last 24h? (Ignore if yes)          │
│  • Atomic Reservation: INSERT ... status='EVALUATING'                  │
│    (Partial unique index prevents concurrent Tokio multi-buys)         │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 4: CLUSTER CONFLUENCE TRIGGER (Rust)                 │
│                                                                        │
│  • Query 12-hour window: Distinct smart BUY wallets >= 3?              │
│  • Subset Temporal Dispersion: Any triplet with span >= 180s, gap >= 5s│
│  • Symmetric Drift Guard: 0.90 * VWAP_SOL <= price <= 1.15 * VWAP_SOL  │
│  • Liquidity & CLMM Guard: DexScreener TVL >= $100k & Impact < 1.5%    │
│  • Update cluster_executions to status='ACTIVE' with cluster_wallets   │
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
│             PHASE 6: ANCHORED SWING POSITION MONITOR (Rust)            │
│                                                                        │
│  • Disaster Stop: -20.0% hard stop                                     │
│  • Anchored Disinvestment: If >= 2 constituent cluster wallets dump    │
│    >= 70% of tokens (anchored to entry - 12h), liquidate position now  │
│  • Tiered Profit Targets (0.25 SOL): 50% at +35%, 50% at +75%         │
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
**Goal:** Provide durable storage for all smart-money transactions with bilateral support (`BUY` / `SELL`), full `numeric` precision VWAP, constituent wallet tracking, and atomic reservation partial indexes.

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
  INCLUDE (wallet_address, amount_sol, amount_tokens, price_sol);

  CREATE INDEX IF NOT EXISTS idx_smart_signals_wallet_time 
  ON smart_money_signals(wallet_address, block_time DESC);

  CREATE TABLE IF NOT EXISTS cluster_executions (
      id BIGSERIAL PRIMARY KEY,
      token_address TEXT NOT NULL,
      cluster_vwap_usd NUMERIC(30, 18) NOT NULL,
      cluster_vwap_sol NUMERIC(30, 18) NOT NULL,
      cluster_wallets TEXT[] NOT NULL,
      wallet_count INT NOT NULL,
      entered_at TIMESTAMPTZ DEFAULT NOW(),
      status TEXT NOT NULL DEFAULT 'EVALUATING' CHECK (status IN ('EVALUATING', 'ACTIVE', 'CLOSED', 'EXPIRED'))
  );

  -- Cooldown index covering both ACTIVE and CLOSED trades
  CREATE INDEX IF NOT EXISTS idx_cluster_executions_cooldown 
  ON cluster_executions(token_address, entered_at DESC);

  -- Partial unique index for atomic reservation (prevents concurrent double-buys without locking connections)
  CREATE UNIQUE INDEX IF NOT EXISTS idx_cluster_executions_active_lock 
  ON cluster_executions(token_address) 
  WHERE status IN ('EVALUATING', 'ACTIVE');

  -- Global cluster view with full precision base-token weighting
  CREATE OR REPLACE VIEW v_active_clusters_12h AS
  SELECT 
      token_address,
      COUNT(DISTINCT wallet_address) AS distinct_wallets,
      SUM(amount_sol) AS total_sol_volume,
      SUM(amount_tokens) AS total_token_volume,
      MIN(block_time) AS first_signal_at,
      MAX(block_time) AS last_signal_at,
      (SUM(price_usd * amount_tokens) / NULLIF(SUM(amount_tokens), 0)) AS vwap_price_usd,
      (SUM(amount_sol) / NULLIF(SUM(amount_tokens), 0)) AS vwap_price_sol,
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
        (SUM(wallet_sol) / NULLIF(SUM(wallet_tokens), 0)) AS vwap_price_sol,
        ARRAY_AGG(wallet_address ORDER BY first_buy_at ASC) AS distinct_wallets_list,
        ARRAY_AGG(first_buy_at ORDER BY first_buy_at ASC) AS distinct_wallet_arrivals
    FROM wallet_first_buys;
    ```
  - Implement `is_token_in_cooldown(&self, token_address: &str, cooldown_hours: i64) -> AppResult<bool>`:
    ```sql
    SELECT EXISTS (
        SELECT 1 FROM cluster_executions
        WHERE token_address = $1
          AND entered_at > NOW() - (INTERVAL '1 hour' * $2)
    );
    ```
  - Implement atomic reservation:
    `try_reserve_cluster_evaluation(&self, token_address: &str) -> AppResult<Option<i64>>`:
    ```sql
    INSERT INTO cluster_executions (token_address, status, entered_at, cluster_vwap_sol, cluster_vwap_usd, cluster_wallets, wallet_count)
    VALUES ($1, 'EVALUATING', NOW(), 0, 0, '{}', 0)
    ON CONFLICT (token_address) WHERE status IN ('EVALUATING', 'ACTIVE') DO NOTHING
    RETURNING id;
    ```
  - Implement `confirm_cluster_execution(&self, id: i64, vwap_sol: Decimal, vwap_usd: Decimal, wallets: &[String]) -> AppResult<()>`.
  - Implement `cancel_cluster_evaluation(&self, id: i64) -> AppResult<()>`.
  - Implement anchored disinvestment holdings query:
    `get_cluster_disinvestment_holdings(&self, token_address: &str, cluster_wallets: &[String], lookback_anchor: DateTime<Utc>) -> AppResult<Vec<WalletHoldingSummary>>`:
    ```sql
    SELECT 
        wallet_address,
        COALESCE(SUM(amount_tokens) FILTER (WHERE side = 'BUY'), 0) AS tokens_bought,
        COALESCE(SUM(amount_tokens) FILTER (WHERE side = 'SELL'), 0) AS tokens_sold,
        COALESCE(SUM(amount_tokens) FILTER (WHERE side = 'SELL'), 0)::numeric / 
            NULLIF(SUM(amount_tokens) FILTER (WHERE side = 'BUY'), 0) AS dump_ratio
    FROM smart_money_signals
    WHERE token_address = $1 
      AND wallet_address = ANY($2)
      AND block_time >= $3
    GROUP BY wallet_address;
    ```

---

### Phase 3: Decoupled Ingestion & Atomic Reservation Concurrency
**Goal:** Persist every raw smart-money swap immediately. Prevent race conditions and duplicate orders without holding open DB connections.

- [ ] **Task 3.1: Ingestion Pipeline in `operator/src/handlers/monitoring.rs`**
  - On incoming swap from Helius:
    1. Extract token, wallet, side (`BUY` or `SELL`), amount_sol, amount_tokens, price_usd, price_sol, slot, signature, timestamp.
    2. Immediately call `db.record_smart_money_signal(...)` asynchronously.
    3. If `side == SELL`:
       - Check if an active Chimera position exists for this token.
       - If yes, pass sell event to `PositionManager::handle_cluster_sell_event(token, wallet)`.
       - Return `Ok(200)`.
    4. If `side == BUY`:
       - Check `db.is_token_in_cooldown(token, 24)`. If locked, return `Ok(200)`.
       - Query `db.get_token_cluster_metrics(token, 12)`.
       - If `distinct_wallets < 3`: Return `Ok(200)` (holding state).
       - If `distinct_wallets >= 3`:
         - Verify **Subset Temporal Dispersion** via `validate_temporal_dispersion(&arrivals)`.
         - Attempt atomic reservation: `db.try_reserve_cluster_evaluation(token)`.
           - If `None`, another worker is already evaluating; return `Ok(200)`.
           - If `Some(reservation_id)`, proceed to admission evaluation.

---

### Phase 4: Cluster Admission, Symmetrical Drift, & CLMM Slippage Gates
**Goal:** Ensure entry only occurs within safe price bounds and verified pool liquidity.

- [ ] **Task 4.1: Selection Gates in `operator/src/engine/selection.rs`**
  - **Cluster Confluence Gate:** Strictly require $\ge 3$ distinct wallets with valid subset dispersion.
  - **Symmetric Entry Drift Guard (SOL Denominated):**
    - Fetch current Jupiter quote for price (Token/SOL):
    - If `current_price_sol > 1.15 * vwap_price_sol` $\to$ Cancel reservation & reject with `CLUSTER_ENTRY_DRIFT_EXCEEDED`.
    - If `current_price_sol < 0.90 * vwap_price_sol` $\to$ Cancel reservation & reject with `CLUSTER_ENTRY_PRICE_COLLAPSED`.
  - **Liquidity Floor:** Check `token_metadata_client.get_liquidity(token) >= $100,000` (DexScreener cached).
  - **CLMM Slippage / Price Impact Guard:**
    - Query Jupiter Quote API for 0.5 SOL swap.
    - If `quote.price_impact_pct > 1.5%` $\to$ Cancel reservation & reject with `CLMM_OUT_OF_RANGE_IMPACT_HIGH`.
  - **Token Age Floor:** $\ge 24\text{ hours}$ old.
  - On successful trade fill, call `db.confirm_cluster_execution(reservation_id, vwap_sol, vwap_usd, &distinct_wallets)`.

---

### Phase 5: Position Sizing & Swing Exit Engine
**Goal:** Provide room to breathe through Solana volatility, curb partial-exit fee drag, and enforce anchored disinvestment exits.

- [ ] **Task 5.1: Sizing in `operator/src/engine/position_sizer.rs`**
  - Base cluster size: `0.25 SOL` (tokens with $\$100\text{k}$–$\$250\text{k}$ liquidity).
  - High-conviction boost: `0.50 SOL` (tokens with $>\$250\text{k}$ liquidity or $\ge 4$ wallets).
  - Portfolio Cap: Maximum 4 concurrent open positions (Max portfolio risk: 2.0 SOL).
  - **Jito Tip Ceiling:** Cap tip at $\le 0.0005\text{ SOL}$ for $0.25\text{ SOL}$ orders, and $\le 0.001\text{ SOL}$ for $0.50\text{ SOL}$ orders.
- [ ] **Task 5.2: Fee-Optimized Swing Exit Parameters in `core/src/config.rs` & `operator/src/engine/exit_rules.rs`**
  - `recovery_gate_enabled = false` (eliminate the 15-minute loss cut).
  - `hard_stop_loss_pct = -20.0%` (wide disaster stop).
  - **For base size ($\le 0.35\text{ SOL}$):**
    - `profit_targets = [35.0, 75.0]` (2 legs: 50% at +35%, 50% at +75%).
    - `target_fractions = [0.50, 0.50]`.
    - Eliminates micro-partial fee drag.
  - `trailing_stop_activation_pct = +40.0%`, with `15.0%` trailing pullback.
  - `max_hold_time_hours = 72`: If position is in profit, convert to Breakeven stop (+0.5%) with 5% trail; if flat/loss, market sell.
- [ ] **Task 5.3: Anchored Disinvestment Exit in `operator/src/engine/position_manager.rs`**
  - When `PositionManager::handle_cluster_sell_event(token, wallet)` is called:
    - Retrieve active position and its originating `cluster_wallets` and `entered_at`.
    - Query `db.get_cluster_disinvestment_holdings(token, &cluster_wallets, entered_at - 12h)`.
    - Count wallets where `dump_ratio >= 0.70`.
    - If $\ge 2$ cluster constituent wallets have `dump_ratio >= 0.70`:
      - Trigger **Emergency Cluster Disinvestment Market Sell**.
      - Log: `"Cluster invalidation: 2+ constituent smart wallets dumped >= 70% of accumulated tokens on {token}. Liquidating position."`

---

## 4. Verification & Testing Plan

### 4.1 Automated Tests
1. **Mathematical VWAP Unit Tests (`infra/tests/vwap_calculation_test.rs`):**
   - Verify volume weighting matches $\sum(P \times V) / \sum V$, retaining full numeric precision.
2. **Subset Temporal Dispersion Tests (`operator/tests/unit/temporal_dispersion_tests.rs`):**
   - 3 wallets spanning 35 seconds $\to$ Rejected ($<180\text{s}$).
   - 4 wallets where Wallet 4 buys 2s after Wallet 3, but Wallets 1, 2, 3 span 200s with $\ge 5$s gaps $\to$ **Approved** (greedy DoS protection verified).
3. **Atomic Reservation Tests (`operator/tests/integration/atomic_reservation_tests.rs`):**
   - Concurrent tasks attempt `try_reserve_cluster_evaluation` for same token $\to$ Exactly 1 succeeds, 2nd receives `None`.
   - After reservation is confirmed `ACTIVE`, concurrent inserts still return `None`.
   - After 24 hours or `CLOSED`, new reservation succeeds.
4. **Anchored Disinvestment Lookback Tests (`operator/tests/integration/cluster_exit_tests.rs`):**
   - Position open for 16 hours. Wallets A and B dump tokens bought 16 hours ago $\to$ Correctly detected as $\ge 70\%$ dump and triggers emergency exit (rolling window bug solved).
5. **Cooldown Re-Entry Tests (`operator/tests/integration/cooldown_tests.rs`):**
   - Trade closed after 30 minutes $\to$ `is_token_in_cooldown` returns `true` for next 23.5 hours.

### 4.2 Production Server Verification (`chimera-01.moez.tech`)
1. Create and deploy branch `engine/cluster-accumulation`.
2. Apply database migration `infra/migrations_postgres/0023_smart_money_clusters.sql`.
3. Verify `smart_money_signals` table captures both `BUY` and `SELL` swaps.
4. Verify paper trades trigger only on valid $\ge 3$ wallet clusters with no re-entry duplicates.
5. Track hold durations and monitor `/api/v1/profitability/verdict`.
