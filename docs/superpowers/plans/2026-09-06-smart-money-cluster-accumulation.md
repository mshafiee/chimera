# Smart Money Cluster Accumulation Engine Implementation Plan

**Date:** 2026-09-06  
**Status:** APPROVED FOR IMPLEMENTATION  
**Branch Base:** `main` (Legacy archived at `archive/v1-copy-trading-legacy`)  
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

### 1.3 Core Axioms of the New Engine
1. **Strict Minimum of $\ge 3$ Distinct Wallets:** 2-wallet pairs perform terribly (−13.31%) because they are frequently a dev + burner wallet or wash-trading bots. Confluence must require $\ge 3$ distinct smart wallets.
2. **Temporal Dispersion Guard:** The 3 smart wallets must not trade in the same 30 seconds (preventing single-actor sybil execution). Accumulation must occur across minutes to hours.
3. **Established Liquidity ($>\$100\text{k}$):** Strictly ban pre-graduation bonding curves and micro-caps.
4. **Swing Horizon (12h–72h):** Kill 5-minute micro stop-losses. Use wide disaster stops (−20%) and asymmetric profit targets (+35%, +75%, +150%).

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
                                    │ Webhook swaps
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 2 & 3: INGESTION & DECOUPLED PERSISTENCE (Rust)      │
│                                                                        │
│  • Ingest swap at /api/v1/monitoring/helius-webhook                    │
│  • Immediately persist to `smart_money_signals` table                  │
│    (No admission gating here — breaks legacy chicken-and-egg deadlock) │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 4: CLUSTER CONFLUENCE TRIGGER (Rust)                 │
│                                                                        │
│  • Query 12-hour window on token: Distinct smart wallets >= 3?         │
│  • Temporal dispersion: Span between first & last wallet >= 30s?       │
│  • Distinct wallets < 3: Record cluster progress point (NO TRADE)      │
│  • Distinct wallets >= 3: Emit CLUSTER_CONFIRMED event                 │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 5: ADMISSION & JITO EXECUTION (Rust)                 │
│                                                                        │
│  • Liquidity Gate: Hard floor >= $100,000 pool liquidity               │
│  • Token Age Gate: >= 24 hours old (No fresh curves)                   │
│  • Price Drift Guard: Current price <= 15% above cluster entry VWAP    │
│  • Execution: Jito Bundle via Jupiter (0.25 - 0.50 SOL)                │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│             PHASE 6: SWING POSITION MONITOR (Rust)                     │
│                                                                        │
│  • Disable 15-minute `recovery_gate` stop chop                         │
│  • Wide Disaster Stop: -20.0%                                          │
│  • Tiered Profit Scaling: +35% (sell 33%), +75% (sell 33%), +150% (34%)│
│  • Trailing Stop: Activate at +40%, trail 15%                          │
│  • Max Hold Time: 72 hours (3 days)                                    │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Detailed Work Breakdown Structure (WBS)

### Phase 1: Scout Swing Roster Seeding (Python)
**Goal:** Ensure the engine tracks 50–100 genuinely active smart-money swing wallets. Currently, the database has only 5 ACTIVE wallets and all have `last_trade_at = NULL`.

- [ ] **Task 1.1: Swing Archetype & Liquidity Filters in `scout/core/analyzer.py`**
  - In `determine_archetype`:
    - Require `avg_hold_time >= 14400` (4+ hours) for `TraderArchetype.SWING`.
    - Disqualify wallets trading on tokens with $< \$100\text{k}$ liquidity.
  - In `analyze_wallet`:
    - Require 30-day realized win rate $\ge 55\%$ and profit factor $\ge 1.8$.
- [ ] **Task 1.2: Roster Discovery & Seeding Script `scout/scripts/seed_swing_roster.py`**
  - Query Helius / Dune historical swap activity for top performing swing addresses.
  - Populate `wallets` table with 50–100 validated wallets under status `ACTIVE_SWING`.
  - Update Helius webhook configuration via `infra/src/monitoring/webhook_manager.rs` or script to subscribe to all 50–100 addresses.

---

### Phase 2: Database Schema & Persistence Layer
**Goal:** Provide durable storage for all smart-money transactions across restarts and compute cluster metrics.

- [ ] **Task 2.1: PostgreSQL Migration `0025_smart_money_clusters.sql`**
  ```sql
  CREATE TABLE IF NOT EXISTS smart_money_signals (
      id BIGSERIAL PRIMARY KEY,
      wallet_address TEXT NOT NULL,
      token_address TEXT NOT NULL,
      token_symbol TEXT,
      amount_sol NUMERIC(30, 18) NOT NULL,
      price_usd NUMERIC(30, 18),
      tx_signature TEXT UNIQUE NOT NULL,
      block_time TIMESTAMPTZ NOT NULL,
      created_at TIMESTAMPTZ DEFAULT NOW()
  );
  CREATE INDEX idx_smart_signals_token_time ON smart_money_signals(token_address, block_time DESC);
  CREATE INDEX idx_smart_signals_wallet_time ON smart_money_signals(wallet_address, block_time DESC);

  CREATE OR REPLACE VIEW v_active_clusters_12h AS
  SELECT 
      token_address,
      COUNT(DISTINCT wallet_address) AS distinct_wallets,
      SUM(amount_sol) AS total_sol_volume,
      MIN(block_time) AS first_signal_at,
      MAX(block_time) AS last_signal_at,
      ROUND(AVG(price_usd)::numeric, 8) AS vwap_price_usd,
      EXTRACT(EPOCH FROM (MAX(block_time) - MIN(block_time))) AS cluster_duration_secs
  FROM smart_money_signals
  WHERE block_time > NOW() - INTERVAL '12 hours'
  GROUP BY token_address
  HAVING COUNT(DISTINCT wallet_address) >= 3;
  ```
- [ ] **Task 2.2: Data Access in `infra/src/db_abstraction/postgres.rs`**
  - Implement `record_smart_money_signal(&self, signal: &SmartMoneySignal) -> AppResult<()>`.
  - Implement `get_cluster_confluence(&self, token_address: &str, window_hours: i64) -> AppResult<Option<ClusterConfluence>>`.

---

### Phase 3: Ingestion Decoupling (Breaking the Deadlock)
**Goal:** Persist every incoming smart-money signal immediately upon webhook receipt, regardless of whether a trade executes.

- [ ] **Task 3.1: Decoupled Ingestion in `operator/src/handlers/monitoring.rs`**
  - On incoming swap:
    1. Extract token, wallet, amount, signature, and timestamp.
    2. Immediately call `db.record_smart_money_signal(...)` asynchronously.
    3. Check `db.get_cluster_confluence(&token, 12)`.
    4. If `distinct_wallets < 3`:
       - Log: `"Smart money signal recorded for token {token}: {distinct}/3 wallets. Holding."`
       - Return `Ok(200)` without queuing a BUY order.
    5. If `distinct_wallets >= 3`:
       - Check `cluster_duration_secs >= 30` (Temporal Dispersion Guard).
       - Forward aggregated cluster payload to `signal_pipeline.process_signal()`.

---

### Phase 4: Cluster Admission & Drift Gates
**Goal:** Ensure we only enter clusters with confirmed liquidity and before the major markup has already run away.

- [ ] **Task 4.1: Selection Gates in `operator/src/engine/selection.rs`**
  - Require `is_smart_money_cluster == true` (strictly $\ge 3$ distinct wallets).
  - **Liquidity Floor:** Hard rejection if pool liquidity $< \$100,000$.
  - **Token Age Floor:** Token must be $\ge 24\text{ hours}$ old.
  - **Cluster Entry Drift Guard:** Fetch Jupiter quote; if `current_price > 1.15 * cluster_vwap`, reject with `CLUSTER_ENTRY_DRIFT_EXCEEDED` (prevents buying after a $>15\%$ pump has already occurred).
  - Single-wallet BUY signals are permanently rejected.

---

### Phase 5: Position Sizing & Swing Exit Engine
**Goal:** Hold through normal market volatility and capture $+35\%$ to $+150\%$ multi-day trend moves.

- [ ] **Task 5.1: Sizing in `operator/src/engine/position_sizer.rs`**
  - Base cluster size: `0.25 SOL` (for tokens with $\$100\text{k}$–$\$250\text{k}$ liquidity).
  - High-conviction boost: `0.50 SOL` (for tokens with $>\$250\text{k}$ liquidity or $\ge 4$ wallets).
  - Portfolio Cap: Maximum 4 concurrent open positions (Max risk: 2.0 SOL total).
- [ ] **Task 5.2: Swing Exit Parameters in `core/src/config.rs` & `operator/src/engine/exit_rules.rs`**
  - `recovery_gate_enabled = false` (eliminate the 15-minute loss cut).
  - `hard_stop_loss_pct = -20.0%` (wide disaster stop).
  - `profit_targets = [35.0, 75.0, 150.0]`.
  - `target_fractions = [0.33, 0.33, 0.34]`.
  - `trailing_stop_activation_pct = +40.0%`, with `15.0%` trailing pullback.
  - `max_hold_time_hours = 72` (3-day time exit for stagnant positions).

---

## 4. Verification & Testing Plan

### 4.1 Automated Tests
1. **Scout Unit Tests (`scout/tests/test_analyzer.py`):**
   - Verify hold time calculation correctly identifies `SWING` vs `SCALPER`.
   - Verify liquidity filter ignores low-liquidity token gambles.
2. **Database & Cluster Integration Tests (`operator/tests/integration/smart_cluster_tests.rs`):**
   - 1 signal from Wallet A $\to$ cluster count = 1 $\to$ NO trade.
   - 2nd signal from Wallet B within 12h $\to$ cluster count = 2 $\to$ NO trade (prevents 2-wallet trap).
   - 3rd signal from Wallet C within 12h (time delta $>30\text{s}$) $\to$ cluster count = 3 $\to$ **Emits CLUSTER_CONFIRMED**.
   - 3 signals in the exact same second $\to$ **Rejected by Temporal Dispersion Guard**.
   - Duplicate trades from same wallet do not increase distinct wallet count.
3. **Exit Engine Tests (`operator/tests/unit/exit_rules_tests.rs`):**
   - Verify position is NOT closed at 15 minutes at $-3\%$.
   - Verify position exits 33% at $+35\%$ and 33% at $+75\%$.
   - Verify hard stop triggers at $-20\%$.

### 4.2 Production Server Verification (`chimera-01.moez.tech`)
1. Deploy updated containers under `TradeMode::Paper`.
2. Run database migration `0025_smart_money_clusters.sql`.
3. Verify `smart_money_signals` table continuously records incoming swaps.
4. Verify that paper trades only execute on confirmed 3+ wallet clusters.
5. Track hold durations and verify median hold shifts from 5.6 minutes to $>12\text{ hours}$.
6. Monitor `/api/v1/profitability/verdict` for positive statistical convergence.
