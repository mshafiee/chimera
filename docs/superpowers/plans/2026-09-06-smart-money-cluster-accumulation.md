# Smart Money Cluster Accumulation Engine Implementation Plan

**Date:** 2026-09-06  
**Status:** PROPOSED  
**Branch Base:** `main` (Legacy archived at `archive/v1-copy-trading-legacy`)  
**Scope:** Pivot Chimera from reactive 1:1 micro copy-trading to an automated Smart Money Cluster Accumulation Engine.

---

## 1. Executive Summary & Core Hypothesis

### The Problem in Legacy Micro Copy-Trading
Across 233 realized paper trades and 26,458 shadow positions, Chimera demonstrated a statistically significant negative edge:
* **Mean Net Return:** **−2.10% per trade** (95% CI: `[−3.43%, −0.77%]`).
* **Win Rate:** **20.2%** (All-time), with 72% of positions stopped out in $<15$ minutes (median hold: 5.6 minutes).
* **Adverse Selection & Latency:** Buying low-liquidity memecoins 1–2 blocks after the target wallet means consistently buying at local micro-peaks and selling into depleted liquidity pools.
* **The Aggregator Deadlock:** Single-wallet signals were gated on multi-wallet consensus, but signals were only added to the in-memory aggregator *after* admission—making multi-wallet clusters mathematically impossible to form without first admitting an unproven single-wallet signal.

### The Cluster Accumulation Hypothesis
1. **Multi-Wallet Confluence:** A single wallet buying a token is often noise, insider manipulation, or copy-bait. However, when **$\ge 2$ or $\ge 3$ independent, high-reputation smart wallets accumulate the same token within a 12-to-24 hour window**, it indicates coordinated institutional or informed conviction.
2. **Horizon & Liquidity Shift:** Banning pre-graduation bonding curves and micro-caps, focusing only on tokens with **liquidity $> \$250,000$** and holding for **12 hours to 3 days** neutralizes block latency (1–2 blocks) and makes 1% DEX fees negligible against target price moves of $+35\%$ to $+150\%$.

---

## 2. Target System Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│                        SCOUT COLD PATH (Python)                        │
│                                                                        │
│  • Continuous Helius transaction scanning                              │
│  • Archetype Filter: Exclude SCALPER/SNIPER; enforce SWING (>4h hold)  │
│  • WQS Score: >55% win rate, >50% 30d ROI on liquid tokens (>$250k)   │
│  • Roster: 50–100 active swing wallets (status = 'ACTIVE_SWING')       │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Webhook registration
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                      INGESTION & PERSISTENCE (Rust)                    │
│                                                                        │
│  • Helius Webhook Endpoint (/api/v1/monitoring/helius-webhook)         │
│  • Persist ALL raw smart-money BUYs to `smart_money_signals` table     │
│    (Decoupled from trade admission — no chicken-and-egg deadlock)      │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                   CLUSTER CONFLUENCE ENGINE (Rust)                     │
│                                                                        │
│  • Check distinct smart wallets on token within 12h–24h window         │
│  • Distinct wallets < 2: Record point, wait (NO TRADE)                 │
│  • Distinct wallets >= 2: Emit CLUSTER_CONFIRMED event                 │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                  ADMISSION GATES & EXECUTION (Rust)                    │
│                                                                        │
│  • Token Liquidity Gate: Must be >= $250,000                           │
│  • Token Age Gate: Must be >= 24 hours old (No fresh curves)           │
│  • Price Drift Guard: Current price <= 15% above cluster entry VWAP    │
│  • Jito Bundle Execution via Jupiter (Size: 0.25 - 0.50 SOL)           │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                      SWING POSITION MONITOR (Rust)                     │
│                                                                        │
│  • Disable 5-minute micro `recovery_gate` stops                        │
│  • Wide Disaster Stop: -20.0%                                          │
│  • Tiered Take-Profits: +35% (sell 33%), +75% (sell 33%), +150% (trail)│
│  • Trailing Stop: Activate at +40%, trail 15%                          │
│  • Max Hold Time: 72 hours (3 days)                                    │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Detailed Work Breakdown Structure (WBS)

### Phase 1: Database Schema & Persistence Layer
**Objective:** Decouple incoming raw signal recording from order execution, ensuring all smart-money observations persist across container restarts.

- [ ] **Task 1.1: Migration `0025_smart_money_clusters.sql`**
  - Create table `smart_money_signals`:
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
    ```
  - Create cluster view `v_active_clusters_12h`:
    ```sql
    CREATE OR REPLACE VIEW v_active_clusters_12h AS
    SELECT 
        token_address,
        COUNT(DISTINCT wallet_address) AS distinct_wallets,
        SUM(amount_sol) AS total_sol_volume,
        MIN(block_time) AS first_signal_at,
        MAX(block_time) AS last_signal_at,
        AVG(price_usd) AS vwap_price_usd
    FROM smart_money_signals
    WHERE block_time > NOW() - INTERVAL '12 hours'
    GROUP BY token_address
    HAVING COUNT(DISTINCT wallet_address) >= 2;
    ```
- [ ] **Task 1.2: Database Abstraction in `chimera_infra`**
  - Add `record_smart_money_signal(&self, signal: &SmartMoneySignal)` to `Database` trait.
  - Add `get_cluster_confluence(&self, token_address: &str, window_hours: i64) -> AppResult<ClusterConfluenceInfo>` to query distinct wallet counts, average entry price, and participant list.
  - Implement methods in `infra/src/db_abstraction/postgres.rs` and update test mock stubs.

---

### Phase 2: Scout Pipeline Overhaul (Swing Smart Money)
**Objective:** Replace scalper/sniper discovery with genuine multi-day informed swing traders.

- [ ] **Task 2.1: Tighten Scout Archetype & Liquidity Filters in `scout/core/analyzer.py`**
  - In `determine_archetype`:
    - Require `avg_hold_time >= 14400` (4+ hours) for `TraderArchetype.SWING`.
    - Mark wallets that trade pump.fun bonding curves with hold times $<15$ minutes as `TraderArchetype.SCALPER` and disqualify them from promotion.
  - In `analyze_wallet`:
    - Add a liquidity sanity filter: Ignore tokens with pool liquidity $< \$250,000$ when calculating a wallet's win rate and ROI.
    - Require 30-day win rate $\ge 55\%$ and profit factor $\ge 1.8$ on qualified liquid trades.
- [ ] **Task 2.2: Roster Management in `scout/core/shadow_promoter.py`**
  - Only promote wallets with archetype `SWING` or `WHALE` to `ACTIVE_SWING`.
  - Automatically demote wallets with zero trades in 14 days or whose average hold time drops below 2 hours.

---

### Phase 3: Operator Signal Pipeline & Cluster Gating
**Objective:** Eliminate single-wallet live execution and enforce cluster confluence before placing BUY orders.

- [ ] **Task 3.1: Ingestion in `operator/src/handlers/monitoring.rs`**
  - Upon receiving a valid Helius webhook swap:
    1. Parse swap details (token, wallet, amount, signature, timestamp).
    2. Asynchronously call `db.record_smart_money_signal(...)` immediately.
    3. Check `db.get_cluster_confluence(&token, 12)`:
       - If `distinct_wallets < 2`: log INFO (`"Cluster point recorded for token {token}: 1/{threshold} wallets. Holding."`) and return without queuing an order.
       - If `distinct_wallets >= 2`: construct an aggregated `ClusterSignalPayload` and pass to `signal_pipeline.process_signal()`.
- [ ] **Task 3.2: Selection Admission Gates in `operator/src/engine/selection.rs`**
  - Permanently lock `require_consensus_or_proven = true` (or rename to `require_cluster_confluence`).
  - Reject any single-wallet BUY request that lacks verified cluster confluence.
  - **Updated Liquidity Gate:** Hard floor of $\$250,000$ pool liquidity.
  - **Updated Token Age Gate:** Token must be $\ge 24\text{ hours}$ old (blocks pump.fun curve gambles).
  - **Price Drift Guard:** Fetch current Jupiter quote; if `current_price > 1.15 * cluster_vwap`, reject with `CLUSTER_ENTRY_DRIFT_EXCEEDED` (prevents buying after the pump already happened).

---

### Phase 4: Position Sizing & Swing Exit Engine
**Objective:** Give swing trades room to breathe and capture $+35\%$ to $+150\%$ trend expansions while limiting catastrophic downside.

- [ ] **Task 4.1: Conviction Sizing in `operator/src/engine/position_sizer.rs`**
  - 2-wallet cluster: `0.25 SOL` base size.
  - 3+-wallet cluster: `0.50 SOL` conviction boost.
  - Max concurrent open positions: capped at 4 (maximum total exposure: 2.0 SOL).
- [ ] **Task 4.2: Swing Exit Configuration in `core/src/config.rs` & `operator/src/engine/exit_rules.rs`**
  - Update default `ProfitManagementConfig`:
    - `recovery_gate_enabled = false`: Disable the 15-minute loss-cutting gate.
    - `hard_stop_loss_pct = -20.0%`: Wide disaster stop to absorb normal crypto volatility.
    - `profit_targets = [35.0, 75.0, 150.0]`: Multi-tiered profit scaling.
    - `target_fractions = [0.33, 0.33, 0.34]`: Scale out 1/3 at each tier.
    - `trailing_stop_activation_pct = +40.0%`, with `trailing_stop_distance_pct = 15.0%`.
    - `max_hold_time_hours = 72`: Time stop at 3 days if price is flat ($\pm 5\%$).

---

## 4. Verification & Testing Strategy

### 4.1 Unit & Integration Tests
* **Rust Database Tests:**
  * Seed 1 smart-money signal $\to$ verify cluster count is 1.
  * Seed 2nd signal from different wallet within 12h $\to$ verify cluster count is 2.
  * Seed 2nd signal from *same* wallet $\to$ verify distinct wallet count remains 1.
  * Seed 2nd signal 13 hours later $\to$ verify cluster window expires first signal.
* **Rust Selection Gates:**
  * Verify single-wallet BUY request is strictly rejected with `REQUIRES_CLUSTER_CONFLUENCE`.
  * Verify 2-wallet cluster with $\$300\text{k}$ liquidity and $<10\%$ drift is admitted.
  * Verify token with $\$80\text{k}$ liquidity is rejected with `LIQUIDITY_BELOW_MINIMUM`.
* **Python Scout Tests:**
  * Verify `analyzer.py` flags a wallet with 15-minute average hold as `SCALPER`.
  * Verify `analyzer.py` flags a wallet with 8-hour average hold as `SWING`.

### 4.2 Paper Trading Forward Test (Server Deployment)
* Deploy updated container images to `chimera-01.moez.tech` in Paper Trading mode.
* Run migration `0025_smart_money_clusters.sql`.
* Monitor for 7–14 days:
  * Verify that raw webhook signals continuously populate `smart_money_signals`.
  * Confirm that live paper trades only trigger when genuine 2+ wallet confluence occurs on liquid tokens.
  * Monitor position hold durations (target: $>12\text{ hours}$ vs legacy 5.6 minutes).
  * Validate profitability verdict via `/api/v1/profitability/verdict`.
