# Chimera — Complete Architecture & Configuration Guide

> High-Frequency, Fault-Tolerant Copy-Trading Platform for Solana.
> This document is the single reference for how the system works, every subsystem,
> and every configuration option.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [High-Level Architecture](#2-high-level-architecture)
3. [Components & Repository Layout](#3-components--repository-layout)
4. [End-to-End Data Flow](#4-end-to-end-data-flow)
5. [Strategies & Trading Modes](#5-strategies--trading-modes)
6. [The Signal Pipeline & Selection Gates (Hot Path)](#6-the-signal-pipeline--selection-gates-hot-path)
7. [Execution, Jito & Jupiter](#7-execution-jito--jupiter)
8. [Position Management & Exit Rules](#8-position-management--exit-rules)
9. [Risk Controls: Circuit Breakers & Kill Switches](#9-risk-controls-circuit-breakers--kill-switches)
10. [The Scout Cold Path (Wallet Intelligence)](#10-the-scout-cold-path-wallet-intelligence)
11. [Data Model (PostgreSQL)](#11-data-model-postgresql)
12. [HTTP / WebSocket API Surface](#12-http--websocket-api-surface)
13. [Web Dashboard](#13-web-dashboard)
14. [Security Architecture](#14-security-architecture)
15. [Resilience & Fault Tolerance](#15-resilience--fault-tolerance)
16. [Deployment (Docker Compose, Profiles, Makefile)](#16-deployment-docker-compose-profiles-makefile)
17. [Configuration Reference — Operator (config.yaml + CHIMERA_*)](#17-configuration-reference--operator-configyaml--chimera_)
18. [Scout Configuration Reference (SCOUT_* env vars + CLI)](#18-scout-configuration-reference-scout_-env-vars--cli)
19. [Observability & Operations](#19-observability--operations)
20. [Testing & Versioning](#20-testing--versioning)

---

## 1. Executive Summary

**Chimera** is an automated copy-trading platform for Solana. It watches a curated roster of
"smart money" wallets on-chain; when one buys or sells a token, Chimera validates the signal
against a deep stack of safety and profitability gates and (in live modes) executes a mirrored
trade through Jupiter, usually inside a Jito bundle.

Two design pillars:

- **Hot/Cold split.** A Rust *hot path* (`api` + `operator` crates) handles the latency-critical
  signal→execution pipeline with sub-5ms internal latency. A Python *cold path* (`scout/`)
  continuously researches, scores, promotes and demotes the wallets the hot path is allowed to
  copy. Neither path can corrupt the other.
- **Barbell strategy.** Capital is split between **Shield** (capital preservation: proven
  wallets, strict liquidity/cost gates) and **Spear** (asymmetric upside: high-conviction
  signals, Jito bundles for guaranteed block inclusion, hunting 50x–100x outliers).

Non-negotiable engineering rules enforced repo-wide:

- Everything touching money uses exact decimals (`rust_decimal::Decimal` in Rust, `Decimal`
  in Python via `scout/core/decimal_utils.py`) — never floats.
- PostgreSQL is the only production database (SQLite decommissioned 2026-07; dev-only).
- Git is the deployment source of truth: commit → push → server `git pull` → compose build/up.
  Never scp binaries.
- Fail-closed defaults everywhere: trade mode defaults to `Paper` (never silently `Live`),
  a missing HMAC secret refuses startup, tokens unindexed by DexScreener count as $0 liquidity.

> **Infrastructure requirement:** servers must be physically near the RPC (Ashburn, VA or
> Amsterdam; RPC latency < 50 ms). 100 ms+ round-trips defeat the hot path and cause
> blockhash-expiry failures.

---

## 2. High-Level Architecture

```
 Helius Enhanced Webhooks            COLD PATH — Python, minutes/hours cadence
 (events for tracked wallets)       ┌──────────┐  WQS v2   ┌──────────────────┐
        │                           │  SCOUT   │─scoring──▶│ Pre-promotion    │
        ▼                           │ discovery│           │ backtest +       │
 /monitoring/helius-webhook         │ + analysis│          │ walk-forward     │
 + RPC polling + LaserStream WSS    └────┬─────┘           └────────┬─────────┘
        │                                │ promote / demote         │
        ▼                                ▼                          ▼
 Signal parse & normalize        ┌─────────────────────────────────────────┐
        │                        │ wallets roster: ACTIVE / CANDIDATE /    │
        ▼                        │ REJECTED  + Helius webhook lifecycle    │
 Exit triggers (stop-loss,       └───────────────────┬─────────────────────┘
  profit targets, time exit,     HOT PATH — Rust, milliseconds cadence     │
  wallet SELL copy)              ┌───────────────────▼─────────────────────┐
        │                        │ SIGNAL PIPELINE                         │
        ▼                        │ HMAC webhook /api/v1/webhook + Helius   │
 Position feed                   │  → rate limit → queue-depth → token     │
                                 │    safety → wallet status → selection   │
                                 │    gates → priority queue → EXECUTOR    │
                                 └─────────────────────┬───────────────────┘
                                                       ▼
                                 EXECUTION: Jupiter swap-v2 quote → tx build
                                 → Jito bundle or standard send → confirm
                                                       ▼
                                 POSITION TRACKING: PnL, price cache, WS
                                 broadcast, DB write-queue, reconciliation
                                                       │ REST + WS (:8080)
                                                       ▼
                                 ┌────────────────┐   ┌─────────────────────┐
                                 │ Web dashboard  │   │ Prometheus/Grafana/ │
                                 │ React SPA :80  │   │ Alertmanager        │
                                 └────────────────┘   └─────────────────────┘
```


Supporting services: **PostgreSQL 15** (single source of truth: roster, trades, positions),
**Redis 7** (shared cache for operator + scout), **Tor+privoxy sidecar** (egress fallback for
Jupiter when no valid API key is configured), and the **Prometheus/Grafana/Alertmanager**
observability stack. All run as docker-compose services on a private `chimera-network`;
Postgres binds to `127.0.0.1:5432` only and is never internet-reachable.

| Path | Latency cadence | Responsibilities |
|---|---|---|
| Hot path (Rust) | ms | Signal ingestion, validation, selection gates, execution, exits, position tracking |
| Cold path (Python) | minutes–hours | Wallet discovery, WQS scoring, backtesting, promotion/demotion, webhook lifecycle, ML validation |
| Dashboard (TS/React) | interactive | Monitoring, wallet/config control, circuit-breaker control, CSV/PDF exports |

---

## 3. Components & Repository Layout

### 3.1 Rust workspace (`Cargo.toml` members: api, core, infra, operator)

| Crate | Package | Role |
|---|---|---|
| `core/` | `chimera_core` | Domain-pure foundation: `AppConfig` (all operator configuration structs, defaults, validation in `config.rs`), models, price cache, roster helpers, retry, Helius quota accounting, Jupiter helpers, experiment/tracer logic. No DB drivers, no web framework. |
| `infra/` | `chimera_infra` | Adapters: PostgreSQL backends (`db_abstraction/`), token-safety parser (`token/`), notifications (Telegram, Discord), monitoring tasks (webhook health/lifecycle, Helius client, signal aggregator), DB `write_queue`, encrypted keypair vault (`vault.rs`), Jupiter HTTP client, `migrations_postgres/0001_full_schema.sql`. |
| `operator/` | `chimera_operator` | Execution hot path library: `engine/` (executor, signal_pipeline, selection, position_sizer, stop_loss, exit_rules, exit_profile, profit_targets, smart_exit, jito_searcher, entry_confirmation, shadow_trader, shadow_fill, onchain_assessment, dust_runner, execution_lock, reconciliation, recovery, rent_scavenger, worker_pool, transaction_builder, decision_recorder, dune_monitor), `handlers/` (webhook, signals, scout, health, market, risk, profitability, operations, monitoring, ws, api, auth, webhook_lifecycle), `middleware/` (auth, hmac, rate_limit), `monitoring/` (Helius WSS + RPC polling), `circuit_breaker.rs`. Also a re-export facade so legacy `chimera_operator::*` paths keep working. |
| `api/` | `chimera_api` | Application bootstrap + HTTP/WebSocket server (Axum) that the dashboard consumes (`src/main.rs` declares all routes). The produced binary is still named `chimera_operator` to keep the deploy pipeline unchanged. `src/bin/bootstrap_dune.rs` = one-shot wallet-PnL history seeder. |

Two code centers of gravity worth knowing:
`operator/src/engine/selection.rs` holds the ~25 admission gates and `SelectionConfig`;
`core/src/config.rs` (~4.5k lines) is the authoritative operator config schema;
`api/src/main.rs` (~3.9k lines) wires everything together.

### 3.2 Python cold path — `scout/` (chimera-scout, Python ≥ 3.11)

Entry point `scout/main.py` (CLI + continuous loop, §18). Core modules under `scout/core/`:
`analyzer`/`optimized_analyzer` (wallet history → metrics), `helius_client` (+ Helius credit
tracker), `laserstream_client`/`websocket_client`, `birdeye_client`, `wqs` (Wallet Quality
Score), `backtester`/`copy_backtest`, `validator` (pre-promotion gates), walk-forward
validation, `smart_discovery`/`webhook_discovery`/multi-timeframe discovery,
`clustering`/`cluster_detector`/`cluster_ensemble`, `market_regime_detector`,
`signal_quality_filter`, `stop_loss_optimizer`, `position_sizer`, `feature_store`/
`feature_enrichment`, ML (`model_registry`, `prediction_logger`, `prediction_matcher`,
XGBoost/LightGBM/meta-learner), `shadow_promoter`, `denylist`, `roster_writer_db`, `db`
(psycopg3 pool), `advanced_cache`/`caching` (L1/L2/L3 + Redis), `state_persistence`,
`cost_estimator`, RugCheck integration, `decimal_utils` (money math).
`scout/config.py` (`ScoutConfig`) is the env-var configuration surface;
`scout/config/*.txt` hold seed wallet/token lists (`wallets.txt`, `seed_wallets.txt`,
`active_tokens.txt`, `exchange_funders.txt`).

### 3.3 Dashboard — `web/` (chimera-dashboard)

React 18 SPA (Vite + TypeScript strict + Tailwind + Zustand). Typed API clients in `src/api/*`
mirror the backend contract. Feature areas: dashboard, risk, signals, operations, wallets,
webhooks, reconciliation, performance, consensus, market, auth, config. Vitest unit tests,
Playwright E2E.

### 3.4 Ancillary directories

| Path | Purpose |
|---|---|
| `database/` | PostgreSQL migrations (`migrations_postgresql/`), evaluation schema, Postgres tuning scripts |
| `infra/migrations_postgres/0001_full_schema.sql` | Core operator schema (auto-run at postgres container init) |
| `scout/schema_scout_tables.sql` | Scout tables (auto-run at postgres container init) |
| `docker-compose.yml` + `-haproxy.yml` / `-prod.yml` / `.evaluation.yml` | All services, profiles, env wiring |
| `ops/` | Prometheus/Grafana/Alertmanager configs, backup scripts, incident runbooks |
| `docs/` | PDD, API docs (`core/api.md`, OpenAPI), guides, runbooks, reviews, this file |
| `config.yaml` (+ `config/config.yaml`, `config/experiment.yaml`) | Operator config files (§17) |
| `VERSION` | Single source of truth for unified semver (change only via `make release`) |
| `Makefile` | Cross-language task runner (`make build|test|lint|fmt|dev|preflight|deploy|release`) |
| `scripts/`, `tools/`, `tests/` | Ops scripts (webhook consolidation, diagnostics), repo-level integration/chaos suites |


---

## 4. End-to-End Data Flow

### 4.1 How a wallet gets into the roster (cold path)

1. **Discovery** — Scout finds candidate wallets: recent profitable swappers via Helius
   parsed transactions (multi-timeframe scans: fast 24 h / trending 4 h / deep up to 7 d),
   token-holder scans, seed lists, Telegram collector (optional). Deduplicated against the
   existing roster; balance/activity validation filters dust and dormant wallets.
2. **Analysis** — For each candidate, fetch ≤ `SCOUT_WALLET_TX_LIMIT` SWAP transactions,
   parse round trips (buys vs closes, ROI 7d/30d, win rate, hold time, drawdown, churn,
   archetype classification: scalper / swing / whale / sniper).
3. **Scoring (WQS v2)** — 0–100 Wallet Quality Score with temporal-consistency penalties,
   recency weighting, momentum boost, and (optionally) ML boost (XGBoost/LightGBM/meta-
   learner profitability prediction). Split-WQS mode (chronological 70/30 trade split)
   removes look-ahead bias.
4. **Validation gates** — walk-forward holdout backtest (last 30 % of trades must stay
   profitable), historical-liquidity-aware copy-backtest (with realistic costs: priority fee +
   Jito tip calibrated to production), RugCheck safety, min close-ratio per archetype,
   forbidden archetypes (e.g. SNIPER), max rejection rate, max PnL reduction, drawdown
   fraction, min avg hold time, low-churn enforcement. Fast-track: WQS ≥
   `SCOUT_FAST_TRACK_WQS_THRESHOLD` (80) with ≥1 trade bypasses close-count/walk-forward/
   backtest gates (RugCheck still enforced).
5. **Promotion** — Wallets are written to the `wallets` roster as `ACTIVE`, `CANDIDATE` or
   `REJECTED` (with WQS + confidence + rejection reasons). `SCOUT_EMERGENCY_PAUSE=1` halts
   all promotions instantly.
6. **Webhook lifecycle** — Operator (and Scout's batch script) register Helius enhanced
   webhooks for ACTIVE wallets, clean up webhooks of demoted wallets, and reconcile against
   the Helius account (§10.4). Dune Analytics optionally seeds/monitors wallet PnL.

### 4.2 How a signal becomes a trade (hot path)

1. **Ingress** — Two sources, normalized to the same internal signal:
   - *Push*: Helius enhanced webhooks (per tracked wallet) arrive at
     `POST /api/v1/monitoring/helius-webhook`; external signal providers hit
     `POST /api/v1/webhook` with an HMAC-SHA256 signature (`X-Signature`, timestamped,
     replay-protected).
   - *Pull*: tiered RPC polling + LaserStream WebSocket feed for wallets without webhooks.
2. **Middleware** — weight-based rate limiting (token bucket, weighted by RPC cost),
   queue-depth check (load shedding at `queue.load_shed_threshold_percent`).
3. **Validation** — wallet must be ACTIVE in the roster; token-safety checks (mint/freeze
   authority whitelists, honeypot simulation, DexScreener liquidity ≥ floor, token age ≥
   minimum, denylist, bonding-curve rules).
4. **Selection gates** — ~25 evidence gates (§6.2): WQS floor, consensus-or-proven, t-stat,
   shadow-mirror token gate, entry-drift, pump-chase, averaging-down, velocity, cluster, etc.
   Every decision (admitted **and** rejected) is recorded by the shadow trader as a twin
   evidence book.
5. **Sizing** — Kelly or WQS×confidence formula with boost/penalty multipliers, clamped to
   `[min_size_sol, strategy_max]` (§8.1).
6. **Priority queue** — EXIT > SHIELD > SPEAR; full ⇒ shed lowest priority first.
7. **Execution** — Jupiter swap-v2 quote (multi-DEX comparison, RTSE), transaction build,
   cost/friction gate, execution lock (idempotency via deterministic UUIDv5), then Jito bundle
   (dynamic percentile tip) or standard submission. Confirmation ⇒ position record.
8. **Position tracking** — price cache + tiered polling updates unrealized PnL; WebSocket
   broadcast to the dashboard; all DB writes go through the retrying write-queue.

### 4.3 How a position exits

Any of: wallet SELL copy (if `copy_wallet_sells`), hard stop-loss (−8 %), adaptive stop
(ATR/regime), trailing stop (activate +10 %, trail 10 %), tiered profit targets
[+15/40/80/150 %] selling 50 % each, time exit (12 h; 1 h for losing Spear / Shield variant),
momentum/smart-exit signals, exit-profile (per-token learned parameters), wick protection
(30 s grace before stop-triggered exits). Exits are confirmed against a live sell quote so
phantom profits are never banked. Exit trades get the highest Jito tip tier.

### 4.4 Continuous feedback loops

- **Shadow trader**: records admitted + rejected signals with simulated fills under the same
  exit rails → feeds the mirror gate, proven-wallet evidence, WQS predictiveness validation.
- **Auto-demotion**: wallets whose *copy* performance degrades (t-stat, recency, Dune 24 h
  PnL monitor, on-chain round-trip expectancy audit) are demoted automatically
  (`monitoring.auto_demote_wallets`).
- **Profitability gate**: computes a rolling GO / STOP / INCONCLUSIVE verdict; in Live mode
  non-GO fail-closes new BUY entries (exits always proceed); Paper mode always proceeds so
  evidence keeps accumulating.
- **Circuit breakers** (§9) halt trading on loss/drawdown/streak thresholds and re-arm after
  the cooldown.


---

## 5. Strategies & Trading Modes

### 5.1 The two strategies (barbell)

| | **Shield** (defense) | **Spear** (offense) |
|---|---|---|
| Goal | Capital preservation | Asymmetric upside (50x–100x outliers) |
| Capital share | `strategy.shield_percent` | `strategy.spear_percent` |
| Signal-quality bar | `strategy.shield_signal_quality_threshold` | `strategy.spear_signal_quality_threshold` |
| Liquidity floor | `token_safety.min_liquidity_shield_usd` | `token_safety.min_liquidity_spear_usd` |
| Cost ceiling | `strategy.shield_max_total_cost_percent` | `strategy.spear_max_total_cost_percent` |
| Per-strategy size cap | `position_sizing.shield_max_pct` (fraction of capital) | `position_sizing.spear_max_pct` |

Sizing mode `hybrid` blends WQS × confidence with per-strategy weights; low-WQS wallets
(< `spear_lite_wqs_threshold`, default 40) are admitted only as micro "Spear-lite" positions
capped at `spear_lite_max_size_sol`. Every parameter's file default and production override
value is listed in §16–§17.

### 5.2 Trade modes (`CHIMERA_TRADE_MODE`)

| Mode | Behavior |
|---|---|
| `paper` (default) | Full pipeline, simulated fills at Jupiter quote price, no transactions signed. The shadow book records everything regardless. The profitability verdict never blocks entries — evidence keeps accumulating. |
| `dust_live` | "Hydra dust lane": real on-chain execution at micro size (~0.05 SOL), learning real execution latency/slippage with bounded loss. |
| `live` | Full-size real execution using the vault keypair (`CHIMERA_WALLET__PRIVATE_KEY`, accepts hex / base58 / JSON byte-array). With `profitability_gate.enforce_on_live=true`, any non-GO verdict fail-closes entry BUYs (exits always proceed). |
| `devnet` | Paper behavior for dev environments (optional Jupiter devnet simulation mode). |

Mode changes are **hot-reloadable** via the dashboard and audited to `config_audit`.
Startup is fail-closed: unset mode = `paper`; Live refuses to run without a valid keypair.

---

## 6. The Signal Pipeline & Selection Gates (Hot Path)

### 6.1 Pipeline stages (each fail-closed)

```
webhook / LaserStream WSS / RPC poll → HMAC + replay check → rate limit
 → queue-depth load-shed → parse/normalize → duplicate filter
 → wallet ACTIVE check → token safety → selection gates
 → profitability verdict (live only) → entry confirmation (defer) → sizing
 → cost/friction gate → priority queue (EXIT > SHIELD > SPEAR) → executor
```

Every stage emits a typed rejection reason (`ENTRY_DRIFT_EXCEEDED`,
`SHADOW_MIRROR_INSUFFICIENT`, `POSITION_SIZE_ZERO`, `PUMP_CHASE`, …) recorded in the decision
log and metrics. Admitted **and** rejected signals both land in the shadow-trader twin book —
this twin evidence is how every gate change is validated (and why each gate carries a REVERT
comment in `docker-compose.yml`).


### 6.2 Selection gates (`operator/src/engine/selection.rs`; env prefix `CHIMERA_SELECTION__`)

**Wallet-quality gates**

| Gate | Knobs (prod values) | Meaning |
|---|---|---|
| WQS floor | `MIN_WQS_SCORE` (70); `WQS_TRIAL_ENABLED` / `WQS_TRIAL_MIN_SCORE` | Wallets below the floor are rejected; a trial lane admits slightly lower WQS at micro size. |
| Consensus-or-proven | `REQUIRE_CONSENSUS_OR_PROVEN` (true), `MIN_PROVEN_TRADES` (3), `REQUIRE_PROVEN_POSITIVE_PNL` (true) | A BUY needs ≥2 tracked wallets on the same token **or** ≥3 closed copy-trades with positive 30d copy PnL. Single-wallet/unproven signals are the measured negative-EV class (17/17 wallets net-negative, 2 wins/49 closed trades). |
| Proven recency | `PROVEN_RECENCY_TRADES` (5; 0 disables) | A "proven" wallet's last N closed copy-trades must not be net-negative — long-window aggregates go stale silently. |
| Token-age trial | `TOKEN_AGE_TRIAL_ENABLED`, `TOKEN_AGE_TRIAL_MAX_SIZE_SOL` (0.25) + proven age-waiver floor (6 min) | Proven wallets trade early entries by design; the age gate is waived to a floor that still filters instant rugs. |
| Wallet t-stat | `WALLET_TSTAT_ENABLED`, `_THRESHOLD` (1.645), `_MIN_SAMPLES`, `_WINDOW_DAYS` | Shadow mirror_main PnL must be statistically significant — wallet selection is the dominant copier-profitability factor (11.3% AUC drop when removed). |
| Shadow-total-PnL proven branch | `SHADOW_PROVEN_ENABLED`, `_MIN_SAMPLES`, `_MIN_TOTAL_PNL_SOL` | OR'd with t-stat: captures high-variance moonshot wallets (huge std → low t) with large positive total PnL. |
| Spear-lite | `SPEAR_LITE_WQS_THRESHOLD` (40), `SPEAR_LITE_MAX_SIZE_SOL` (0.25) | Low-WQS wallets admitted only at micro size while accumulating a track record. |
| Smart-money cluster | `cluster_gate_enabled`, `cluster_min_profitable_wallets` | ≥N statistically-profitable wallets buying the same token within 12 h also satisfies the consensus requirement. |

**Token-safety gates** (`CHIMERA_TOKEN_SAFETY__*`, enforced in `infra/src/token/`): mint/freeze
authority whitelists (USDC, USDT, wSOL + Raydium/Orca/pump program IDs), liquidity floors
(prod override $10 k both strategies), `min_token_age_hours` (unknown age = fail-closed for
Spear, warn-and-allow for Shield), honeypot sell simulation, denylist, bonding-curve rules,
`allow_unlisted_heuristic=false` (a token unindexed by DexScreener = $0 liquidity = reject).

**Entry-timing gates**

| Gate | Knobs (prod values) | Meaning |
|---|---|---|
| Entry confirmation | `CHIMERA_ENTRY_CONFIRMATION__{ENABLED,WAIT_SECS,MAX_DRAWDOWN_PCT}` (true / 30 s / 3 %) | Single-wallet unproven BUYs are deferred and admitted only if the price holds within tolerance of the whale's entry — filters instant rugs without buying pump tops (300 s default recalibrated to 30 s after gap measurement). |
| Entry drift guard | `ENTRY_DRIFT_GUARD_ENABLED`, `MAX_ENTRY_DRIFT_PCT` (1.0; code default 3.0) | Reject when our fill would be >X % above the whale's price (twin forensics measured a −2.6 pp paper-vs-shadow execution gap). |
| Pump-chase | `PUMP_CHASE_ENABLED`, `PUMP_CHASE_MAX_DELTA_PCT` | Reject tokens already up >X % in 15 min unless consensus/cluster supports the chase. |
| Averaging-down | `AVG_DOWN_ENABLED`, `AVG_DOWN_{WINDOW_HOURS,MIN_BUYS,MIN_DROP_PCT}` | Reject whale re-buys ≥X % below its first buy — the whale is averaging into a falling knife. |
| Stop-loss re-entry cooldown | `SL_COOLDOWN_ENABLED`, `SL_COOLDOWN_{HOURS,LOSS_PCT}` | Block new BUYs on a token recently stopped out at ≥X % loss. |
| Shadow-mirror token gate | `MIRROR_GATE_ENABLED`, `_MIN_AVG_PCT` (1.5), `_MIN_SAMPLES` (3), `_WINDOW_HOURS` (72) | Admit a token only if the whale's own round trips under our exit rails average ≥ +1.5 % (post-cost breakeven ≈ 1.4 %). Insufficient samples → reject + route to entry confirmation. |
| Liquidity velocity | `TOKEN_VELOCITY_GATE_ENABLED`, `TOKEN_MIN_LIQUIDITY_VELOCITY`, `TOKEN_MAX_CURVE_COMPLETION` | Pump.fun bonding-curve tokens must be in FAST accumulation (velocity = reserves/swap-count) and not in the late-curve dump zone. |

Copy PnL is evaluated over `wallet_copy_pnl_window_days` (30 d). `CHIMERA_SHADOW_MAX_LIFETIME_HOURS`
bounds open shadow positions (prod: 24 h) so the evidence set stays finite.


---

## 7. Execution, Jito & Jupiter

### 7.1 Executor flow (`operator/src/engine/executor.rs`)

1. **Execution lock** (`execution_lock.*`): idempotency — a deterministic key derived from
   (wallet, mint, side, source signature) prevents double-fires on webhook redelivery/restart.
2. **Quote**: Jupiter Swap v2 (`jupiter.api_url = https://api.jup.ag/swap/v2`,
   `use_swap_v2=true`) — the meta-aggregator with RTSE, Jupiter Beam and gasless options;
   `x-api-key` header from `CHIMERA_JUPITER__API_KEY` (required for Live — keyless access is
   being phased out). Without a valid key, egress can route through the Tor/privoxy sidecar.
3. **Cost/friction gate**: total execution cost (tip + DEX fee + price impact/slippage) must
   be ≤ `shield_max_total_cost_percent` / `spear_max_total_cost_percent`, otherwise the trade
   DEAD_LETTERs. Optional Kelly-based friction gating (`friction_gating_enabled`) rejects
   trades where expected edge ≤ friction (disabled in prod so tail micro-trades can fill).
4. **Submit**: with `mev_protection.always_use_jito` (default true) trades go into a **Jito
   bundle** — tip from the dynamic percentile feed (top of block `tip_percentile: 50`),
   clamped `tip_floor_sol` (0.0005) – `tip_ceiling_sol` (0.003) and capped at
   `tip_percent_max` (3 %) of trade size; otherwise standard RPC submission with priority
   fees + blockhash-expiry handling. Tip tiering: exits (0.002) > consensus (0.001) >
   standard (0.0006 SOL). Tips are persisted to `jito_tip_history` and landings audited
   (`jito_searcher`).
5. **Confirm & record**: on confirmation position + trade rows are written via the DB
   write-queue, WebSocket broadcast fires, notifications route per rules.
6. **On-chain assessment** (`onchain_assessment.*`): fills are re-checked against actual chain
   state; `shadow_fill` records simulated fills for the paper/shadow twin book.

### 7.2 Failure paths

Failed executions land in `dead_letter_queue` with the failure reason (visible at
`/api/v1/incidents/dead-letter`); the circuit breaker counts Jupiter failures
(`max_jupiter_failures`); RPC failover switches to `rpc.fallback_url` after
`max_consecutive_failures`; `recovery` + `reconciliation` repair state after restarts (§15).

---

## 8. Position Management & Exit Rules

### 8.1 Sizing (`position_sizing.*`)

- `use_kelly_sizing=true`: Kelly fraction of `total_capital_sol`; otherwise WQS × confidence
  (or `hybrid` blend) formulas.
- Capital-relative caps (2026-08-20 scheme): `base_size_pct` (15 % of capital),
  `max_position_pct` (30 %), `shield_max_pct` / `spear_max_pct` per-strategy caps,
  proven-wallet boost (15 % of capital), portfolio heat ceiling (fraction of capital).
- Absolute knobs still apply: `min_size_sol` (file 0.05; prod override 0.10 after the sizing
  drought — see REVERT comment), `min_live_position_sol` (0.02), `max_size_sol` (2.0),
  `max_concurrent_positions` (8), `base_size_sol` (0.25).
- Multipliers: `consensus_multiplier` (2.0 for ≥2-wallet signals),
  `conviction_size_multiplier` (1.5 for proven wallets — amortizes the ~0.005 SOL round-trip
  fees), `off_hours_size_multiplier` (0.5 during 02:00–06:00 UTC), regime multipliers,
  Spear-lite cap for low-WQS wallets.
- Final size clamped to `[min_size_sol, per-strategy max]`; below the floor the BUY is
  rejected `POSITION_SIZE_ZERO` (never opened as dust).

### 8.2 Exit rules (`profit_management.*`, `exit_profile.*`, `smart_exit`)

| Rule | config.yaml default | Notes |
|---|---|---|
| Copy wallet SELL | `strategy.copy_wallet_sells` (true) | If false, positions close only via the rails below (signal-trading mode). |
| Hard stop-loss | `hard_stop_loss: -8 %` | Adaptive ATR/regime overlays; stop-triggered exits get a 30 s **wick-protection grace** (`wick_protection_secs`). |
| Trailing stop | activate +10 %, trail 10 % | Arms once profit crosses activation. |
| Tiered profit targets | `[+15, +40, +80, +150] %`, sell 50 % of remaining per tier | `tiered_exit_percent: 50`. |
| Time exit | 12 h | Losing Spear positions exit after 1 h (`losing_time_exit_hours_spear`). |
| Exit profiles | `exit_profile.*` | Per-token learned exit parameters (hold-time/behavior clusters). |
| Live-quote confirmation | — | Exits quoted against a live sell quote; phantom take-profit profits are never banked. |

Exits queue at highest priority and get the top Jito tip tier. The Scout-side
`stop_loss_optimizer` tunes ATR multipliers per regime (`SCOUT_STOP_LOSS_*`, §18).


---

## 9. Risk Controls: Circuit Breakers & Kill Switches

### 9.1 Automatic circuit breakers (`circuit_breakers.*`, `operator/src/circuit_breaker.rs`)

| Breaker | File default | Prod override (paper) | Trips when |
|---|---|---|---|
| 24 h loss cap | `max_loss_24h_usd: 1000` | — | Realized 24 h loss exceeds USD cap |
| Consecutive losses | 7 | **10** (`CHIMERA_CIRCUIT_BREAKERS__MAX_CONSECUTIVE_LOSSES`) | Loss streak pauses Spear. 5→10 because a 5-loss streak tripped ~5x/24 h at the current win rate (near-certain at n=16/day), dropping live signals and fragmenting evidence windows; revisit at ≥31 % (break-even) win. |
| Max drawdown | `max_drawdown_percent: 20` | **30** (paper-only band) | Portfolio drawdown % exceeds band. 15→30 for paper so the measurement flow can't latch with zero open positions; REVERT if drawdown exceeds 30 % without verdict-sample improvement. |
| Portfolio stop-loss | `portfolio_stop_loss_percent` | — | Hard portfolio halt |
| Jupiter failures | `max_jupiter_failures` | — | Consecutive provider failures halt execution |
| Cooldown | `cooldown_minutes: 30` | — | Auto re-arm window; a trip baselines the streak (livelock fix) so cooldown can actually expire to Active |

State persists in `circuit_breaker_state`; operators can **trip/reset manually** via
`POST /api/v1/config/circuit-breaker/trip` / `/reset` (dashboard Risk page); every manual
action lands in `config_audit`.

### 9.2 Other kill switches

- **Scout emergency pause**: `SCOUT_EMERGENCY_PAUSE=1` → zero promotions instantly.
- **Profitability gate** (`profitability_gate.*`): rolling GO / STOP / INCONCLUSIVE verdict
  recomputed every `refresh_interval_seconds` (300 s); `enforce_on_live` (default **false** =
  the "live == paper" policy) blocks non-GO live entry BUYs when true;
  `inconclusive_size_factor` (0.5) halves size on INCONCLUSIVE. Paper mode always proceeds so
  shadow evidence keeps accumulating.
- **Rejection mute** (`rejection_mute.*`): wallets whose signals are rejected at excessive
  rates are temporarily muted (stops decision spam + wasted quotes).
- **Shadow token blacklist** (`shadow_blacklist.*`): tokens with persistently negative shadow
  evidence are auto-blacklisted from admission.
- **Wallet degradation auto-demotion** (`monitoring.auto_demote_wallets=true`,
  `degradation.*`): wallets whose *copy* performance degrades (t-stat, recency, Dune 24 h PnL
  monitor, on-chain round-trip expectancy audit) are demoted without human action.
- **Experiment controls** (`experiment.*`): live tracer trades capped by `tracer_cap`, verdict
  after `experiment_days` / `min_trades`, kill if roster toxicity >
  `toxic_threshold_percent` (30 %).

---

## 10. The Scout Cold Path (Wallet Intelligence)

### 10.1 Run cycle (`python scout/main.py --continuous`)

1. **Discovery**: find candidate wallets — recent profitable swappers via Helius parsed
   transactions across timeframes (`SCOUT_DISCOVERY_FAST_HOURS` / `_TRENDING_HOURS` /
   `_DEEP_HOURS`, multi-timeframe mode with goal-driven parallel scans), token-holder and
   seed-list scans, optional Telegram collector; dedup against roster (TTL), balance/activity
   validation (`SCOUT_VALIDATE_WALLET_ACTIVITY`, balance fail-mode), Sybil detection
   (`SCOUT_SYBIL_HOPS`, exchange-funder list), min wallet age (`SCOUT_MIN_WALLET_AGE_DAYS`).
2. **Analysis**: ≤ `SCOUT_WALLET_TX_LIMIT` SWAPs per wallet (paginated, ≤
   `SCOUT_WALLET_TX_MAX_PAGES`); round-trip reconstruction; ROI 7d/30d, win rate, hold time,
   drawdown, churn, close ratio; archetype classification (scalper/swing/whale/sniper);
   liquidity in Shield vs Spear tokens (`SCOUT_MIN_LIQUIDITY_SHIELD/_SPEAR`).
3. **WQS v2 scoring** (`SCOUT_WQS_*`): 0–100 composite with recency weighting
   (`SCOUT_WQS_RECENCY_WEIGHT`), temporal-consistency penalties, momentum boost
   (`SCOUT_MOMENTUM_BOOST`), regime multiplier (bull/bear/volatile/neutral), optional ML boost
   (ensemble XGBoost + LightGBM + meta-learner profitability prediction, capped by
   `SCOUT_MAX_HEURISTIC_BOOST` / `SCOUT_ENSEMBLE_WEIGHT`), optional **Split-WQS** mode
   (`SCOUT_SPLIT_WQS_ENABLED`: chronological 70/30 trade split removes look-ahead bias).
4. **Validation**: walk-forward holdout — last `SCOUT_WALK_FORWARD_HOLDOUT_FRACTION` (30 %) of
   trades must produce ≥ `SCOUT_MIN_HOLDOUT_PNL_SOL` profit, else penalty/demotion;
   historical-liquidity-aware copy backtest with realistic costs (`SCOUT_PRIORITY_FEE_SOL`,
   `SCOUT_JITO_TIP_SOL`, `SCOUT_DEX_FEE_PCT`, entry/exit delay slippage, optional CPMM
   slippage + dynamic fees); RugCheck fail-mode safety (`SCOUT_SAFETY_FAIL_MODE`);
   rejection-rate, drawdown-fraction, low-churn, min-hold-time, forbidden-archetype gates.
5. **Promotion/demotion** → `wallets` roster (ACTIVE / CANDIDATE / REJECTED + WQS +
   confidence + reasons; history in `wallet_performance_history`, `growth_history`,
   `roi_metrics`). Fast-track lane: WQS ≥ `SCOUT_FAST_TRACK_WQS_THRESHOLD` (80) with ≥
   `SCOUT_FAST_TRACK_MIN_TRADES` (5). Demotion on degradation, stale performance, or emergency
   pause.


6. **Correlation & research loops**: `wqs_pnl_correlation` Pearson-correlates WQS vs realized
   copy PnL (`make validate-wqs`, `make backfill-correlation`); shadow-promoter dual-write
   (`SCOUT_WQS_COMPARISON_MODE`) A/B-tests scoring variants; ML prediction log → matcher →
   validation report (`make validation`); drift detection + auto-retrain
   (`SCOUT_DRIFT_DETECTION_ENABLED`, `SCOUT_AUTO_RETRAIN_ENABLED`, `SCOUT_RETRAIN_INTERVAL_HOURS`).
7. **Webhook lifecycle batch**: deterministic batch registration/cleanup of Helius webhooks for
   ACTIVE wallets (standing decision: per-wallet registration was quota-blocked and failed
   repeatedly, so coverage is a batch script), coordinated with the operator's
   `webhook_lifecycle` manager (§17).

### 10.2 Budget & rate discipline

Helius credits are a hard budget: `SCOUT_MONTHLY_CREDITS`, `SCOUT_TOTAL_ANALYSIS_CREDITS`,
`SCOUT_MAX_API_CALLS_PER_RUN`, persisted credit history, adaptive rate limiting
(`SCOUT_RATE_LIMIT_ADAPTIVE` + min/max delay + `SCOUT_MAX_REQUESTS_PER_SECOND`/`SCOUT_TARGET_RPS`),
circuit breaker around Helius (`SCOUT_CIRCUIT_BREAKER_*`), quota probe, tiered wallet caps
(`SCOUT_MAX_WALLETS_TIER1/_TIER2`), and a three-level cache — L1 memory, L2 disk, L3 Redis —
with WQS-aware TTLs (`SCOUT_CACHE_*`). Production cadence: `--continuous` every 7200 s.

### 10.3 Scout CLI (summary — full list in §18.2)

`--max-wallets`, `--dry-run`, `--skip-backtest`, `--continuous [--continuous-interval]`,
`--min-wqs-active/--min-wqs-candidate`, `--min-liquidity-shield/-spear`,
`--priority-fee-sol`, `--jito-tip-sol`, `--walk-forward-min-trades`, `--discovery-hours`,
`--wallet-tx-limit`, `--wallet-tx-max-pages`, `--clear-cache`, `--calibration-report`,
`--verbose`.

---

## 11. Data Model (PostgreSQL)

Two schema owners, both applied automatically at postgres container first-init
(`docker-entrypoint-initdb.d`): `infra/migrations_postgres/0001_full_schema.sql` (operator)
and `scout/schema_scout_tables.sql` (scout). Further migrations in `database/migrations_postgresql/`.

### 11.1 Operator tables (22)

| Table | Purpose |
|---|---|
| `trades` | Every executed/paper trade: side, size, prices, fees, mode, strategy, status, tx signature |
| `positions` | Open/closed positions: entry, size, token, PnL, exit reason, trade FKs |
| `wallets` | The roster: address, status (ACTIVE/CANDIDATE/REJECTED), WQS, confidence, archetype, strategy, webhook id, rejection metadata |
| `dead_letter_queue` | Failed executions/records with failure reasons (replayed by recovery) |
| `config_audit` | Immutable audit log of every config/trade-mode/circuit-breaker change |
| `kill_switch_state` / `circuit_breaker_state` | Persisted breaker state (survives restarts) |
| `admin_wallets` | Admin wallet allowlist for API auth |
| `jito_tip_history` | Tip percentiles + bundle landings (tip calibration source) |
| `reconciliation_log` | DB vs on-chain reconciliation results |
| `backups` | Backup registry (verified by `make backup-verify`) |
| `historical_liquidity` | Liquidity snapshots used by backtests and the velocity gate |
| `wallet_monitoring` | Per-wallet monitoring state: last seen, PnL windows, demotion evidence |
| `exit_targets` | Per-position planned tiered exit levels |
| `signal_aggregation` | Consensus tracking (wallets per token per window) |
| `wallet_copy_performance` | Realized copy-trade PnL per wallet (proven/recency gates read this) |
| `rate_limit_metrics` | Rate-limiter telemetry |
| `webhook_lifecycle_audit` / `webhook_configuration` | Helius webhook registration/cleanup audit + desired state |
| `wqs_pnl_correlation` | WQS vs realized copy-PnL correlation records |
| `schema_migrations` | Migration ledger |

### 11.2 Scout tables (12)

`ml_predictions`, `exit_recommendations`, `alerts`, `metrics`, `health_checks`,
`growth_history`, `capital_events`, `growth_alerts`, `credit_history`,
`wallet_performance_history`, `roi_metrics`, `multi_timeframe_discovery_stats`.

Shadow-trader twin-book rows and decision logs ride alongside `trades`/`positions` (shadow
mode flag) so the admitted vs rejected cohorts can be compared with one query.


---

## 12. HTTP / WebSocket API Surface

All routes live under `/api/v1` (Axum; `api/src/main.rs` registers ~75 routes). Auth tiers:
HMAC-signed webhook (shared secret), API key (`X-API-Key`; roles from `security.api_keys`),
admin wallet challenge/response (`POST /auth/wallet` + `/auth/refresh` JWT refresh), plus open
ingest/read paths where noted. `/metrics` is the Prometheus scrape endpoint (scraped by the
prometheus container over the private network).

| Group | Endpoints |
|---|---|
| Ingest | `POST /webhook` (HMAC signal providers), `POST /monitoring/helius-webhook` (Helius wallet events; validated against the roster rather than HMAC) |
| Auth | `POST /auth/wallet`, `POST /auth/refresh` |
| Core data | `GET /trades`, `GET /trades/export` (CSV), `GET /positions`, `GET /positions/:trade_uuid`, `GET /wallets`, `GET /wallets/:address` (+ wallet status control endpoints used by the dashboard) |
| Config/control | `GET/POST /config` (incl. trade-mode hot reload), `POST /config/circuit-breaker/trip`, `POST /config/circuit-breaker/reset` |
| Scout control | `POST /scout/run`, `GET /scout/status`, `/scout/metrics`, `/scout/budget`, `/scout/cache`, `/scout/conviction`, `/scout/wqs-distribution` |
| Webhook mgmt | `GET /monitoring/status`, `/monitoring/webhooks/stats`, `/monitoring/webhooks/audit` (+ registration endpoints) |
| Market | `GET /market/conditions`, `GET /market/regime` |
| Metrics | `GET /metrics` (Prometheus), `/metrics/strategy`, `/metrics/performance`, `/metrics/costs`, `/metrics/trade-latency`, `/metrics/database-performance`, `/metrics/request-rate` |
| Operations | `/operations/health-checks`, `/operations/rate-limit`, `/operations/resources`, `/operations/secrets` (masked), `/incidents/dead-letter`, `/incidents/config-audit`, `GET /health`, `GET /ping`, `GET /ws` (WebSocket live feed), `/debug/backtest-smoke` |
| Risk/profitability | verdict + risk-status endpoints consumed by the dashboard Risk/Performance pages |

Full per-endpoint contracts: `docs/core/api.md` + `docs/api/openapi.json`.

---

## 13. Web Dashboard

React 18 SPA built with Vite (TypeScript strict, Tailwind, Zustand stores). Served by the
`web` container (nginx) on :80, proxying `/api` to the operator; HA deployment fronts it with
HAProxy + Let's Encrypt TLS (`docker-compose-haproxy.yml`).

Feature areas (`web/src/features/` + `web/src/components/`): **Dashboard** (PnL, open
positions, live signal feed over `/ws`), **Wallets** (roster table, WQS, promote/demote,
webhook status), **Webhooks** (Helius lifecycle + audit), **Signals** (decision log with
rejection reasons), **Risk** (circuit breakers, manual trip/reset), **Operations** (resources,
rate limits, dead-letter, config audit), **Reconciliation**, **Performance / Consensus /
Market**, **Config** (trade-mode switch, live-editable settings), **Auth** (admin wallet
login), shared `ui/` primitives + `charts/`.

All API access goes through typed clients in `web/src/api/*` which mirror the backend
contract; breaking API changes must update both sides (per `web/AGENTS.md`). Testing: Vitest
(unit) + Playwright (E2E).

---


## 14. Security Architecture

| Layer | Mechanism |
|---|---|
| Inbound webhooks | HMAC-SHA256 over body+timestamp (`X-Signature`), ±`max_timestamp_drift_secs` (60 s) replay window, constant-time compare; startup **refuses to run** if `CHIMERA_SECURITY__WEBHOOK_SECRET` is unset; `CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS` enables zero-downtime rotation (accepts either during a window) |
| API auth | API keys with roles (`security.api_keys`), admin wallets (`security.admin_wallets` / `admin_wallets` table), JWT refresh flow |
| Secrets at rest | Encrypted vault (`infra/src/vault.rs`) for the trading keypair (`CHIMERA_WALLET__PRIVATE_KEY`; hex / base58 / JSON byte-array all accepted and parsed downstream); secrets redacted in logs (`[REDACTED]`); `/operations/secrets` returns masked values |
| Rate limiting | Weighted token-bucket middleware (weight ∝ RPC cost) + `security.webhook_rate_limit` (10 000) / burst (15 000); Helius webhook processing rate limit (45/s) |
| Token safety | Fail-closed posture: unindexed token = $0 liquidity = reject (`allow_unlisted_heuristic=false`); mint/freeze-authority checks with narrow whitelists; honeypot simulation |
| Network | Private docker network; Postgres bound to 127.0.0.1 only (never published to the internet); dashboard behind nginx/HAProxy TLS; dev-mode safety bypasses explicitly disabled (`CHIMERA_DEV_MODE=0`) |
| Auditability | `config_audit` (every change), `webhook_lifecycle_audit`, decision recording — all mutations attributable |
| Money math | Decimal-only (no floats) enforced by convention + review (`rust_decimal` / Python `Decimal`) |

---

## 15. Resilience & Fault Tolerance

- **Write queue** (`infra/src/state/write_queue.rs`): all DB writes go through a buffered,
  retrying queue — the hot path never blocks on the DB; persistently failed writes land in
  `dead_letter_queue`.
- **Recovery + reconciliation**: on restart, open positions and pending records are re-synced
  from DB + chain; periodic reconciliation compares DB vs on-chain state and logs to
  `reconciliation_log`; DLQ replay drains the backlog.
- **RPC failover**: primary→fallback provider switch after `max_consecutive_failures`;
  `functional_health_check` probes `getLatestBlockhash` (catches providers that answer
  `getHealth: ok` while broken); per-second rate limiting + retry/backoff in `core::retry`.
- **Load shedding**: priority queue (capacity 1000) sheds lowest priority (SPEAR entries)
  before exits once `load_shed_threshold_percent` (80 %) is crossed.
- **Backpressure & quota defense**: Helius credit budget + adaptive throttling in Scout;
  webhook processing rate limit; documented 429-recovery playbooks (liquidity floors were
  lowered once during a Helius 429 storm — see compose comments).
- **Graceful degradation**: WebSocket reconnect config (`websocket_reconnect`), tiered price
  polling by conviction tier (`tiered_polling`), inactivity rotation of monitored wallets
  (`inactivity_rotation`), degradation monitoring (`degradation.*`) with alerts, Tor egress
  fallback for keyless Jupiter calls, slippage fallbacks
  (`slippage_fallback_small/large_percent` around `slippage_fallback_threshold_sol`) when
  Jupiter price impact is unavailable.
- **Shadow trader**: if execution rails break, the shadow book keeps measuring signal quality —
  evidence collection is independent of execution success.
- **Rent scavenger**: `RENT_SCAVENGER_*` env vars run a periodic closed-position rent recovery
  sweep (batch size + max rent lamports caps) to reclaim SOL from stale token accounts.
- **Operations**: health endpoints + Prometheus alert rules (dead-letter depth, breaker trips,
  stale webhooks, PnL degradation) → Alertmanager; DB backups via `ops/backup-postgres.sh` /
  `make db-backup`, verified by `make backup-verify`, restored via `make rollback`.

---


## 16. Deployment (Docker Compose, Profiles, Makefile)

### 16.1 The deployment workflow (git is the source of truth)

```bash
# 1. Locally: change, commit, push
git add -A && git commit -m "fix: ..." && git push origin main

# 2. On the production server (root@chimera-01.moez.tech), repo at /opt/chimera
git pull origin main
COMPOSE_PROFILE=mainnet-prod docker compose --profile mainnet-prod \
  -f docker-compose.yml -f docker-compose-haproxy.yml build <service>
COMPOSE_PROFILE=mainnet-prod docker compose --profile mainnet-prod \
  -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate <service>
```

**Never scp binaries to the server.** `COMPOSE_PROFILE` selects the env_file
(`${COMPOSE_PROFILE:-devnet}`); the **`--profile` flag is mandatory** — every service carries a
`profiles:` key, so without it compose activates zero services.

### 16.2 Compose services & profiles

| Service | Container | Image / build | Profiles |
|---|---|---|---|
| `postgres` | chimera-postgres | postgres:15-alpine (localhost-only 127.0.0.1:5432; tuning: 2 GB shared_buffers, 50 max conns, WAL replica level) | mainnet-prod, evaluation |
| `redis` | chimera-redis | redis:7 (L3 cache for operator + scout) | mainnet-prod, evaluation |
| `operator` | chimera-operator | Rust workspace (`api` binary, still named `chimera-operator`) | mainnet-paper, mainnet-prod, evaluation |
| `bootstrap-dune` | chimera-bootstrap-dune | one-shot Dune PnL history seeder | on-demand |
| `tor` (+privoxy) | chimera-tor | Tor egress for keyless Jupiter fallback | mainnet-paper, mainnet-prod, evaluation |
| `scout` | chimera-scout | Python 3.11 (`main.py --continuous`) | mainnet-paper, mainnet-prod |
| `web` | chimera-web | nginx serving the built Vite SPA | mainnet-paper, mainnet-prod |
| `prometheus` / `grafana` / `alertmanager` | chimera-* | observability stack (configs in `ops/`) | mainnet-paper, mainnet-prod (alertmanager: paper+prod) |
| `postgres-exporter` / `node-exporter` | chimera-* | Prometheus exporters | mainnet-prod (node also evaluation) |

Volumes: `postgres-data`, `redis-data`, `scout-data`, `prometheus-data`, `grafana-data`,
`alertmanager-data`; network: `chimera-network` (internal). Overlay files:
`docker-compose-haproxy.yml` (TLS front + Let's Encrypt), `-prod.yml`, `.evaluation.yml`.

### 16.3 Production parameter overrides

The production tuning lives in `docker-compose.yml` operator env entries (each with a dated
comment explaining the measurement behind it and a REVERT condition). Key prod values vs file
defaults: trade mode `paper`; consecutive-loss breaker 7→10; drawdown band 20→30 (paper);
sizing `min_size_sol` 0.05→0.10 (drought fix); cost ceilings Shield 0.3 %→1.2 %, Spear 2.5 %,
Kelly friction gating off; liquidity floors →$10 k both strategies; entry confirmation
on/30 s/3 %; entry drift 1.0 %; consensus-or-proven on with 3 proven trades; proven recency
window 5 trades; Spear-lite 40/0.25; mirror gate 1.5 %/3 samples/72 h; t-stat gate on;
velocity/averaging-down/pump-chase gates on; shadow positions capped at 24 h lifetime;
profitability gate enabled. See §17 for the full map.

### 16.4 Makefile entry points

| Target | Purpose |
|---|---|
| `make build` / `build-operator` / `build-web` | Release builds (Rust + Vite) |
| `make test` / `test-operator` / `test-scout` / `test-web` | Unit suites; `test-all` adds `test-integration` + `test-chaos`; `test-load` (k6) |
| `make lint` / `fmt` | clippy + ruff + tsc-eslint / rustfmt + prettier |
| `make dev` / `dev-operator` / `dev-web` / `dev-scout` | Local dev loops |
| `make db-init` / `db-shell` / `db-backup` / `backup-verify` / `rollback` | DB lifecycle |
| `make validate-wqs` / `backfill-correlation` / `validation[-report|-match]` | Scout research loops |
| `make version` / `version-check` / `release TYPE=patch\|minor\|major` | Unified semver (VERSION file → syncs Cargo.toml/pyproject/package.json → git tag) |
| `make preflight` / `deploy` / `deploy-rsync` / `install-service` / `logs[-all]` | Deploy + ops |
| `make audit` / `check-deps` / `clean[-all]` | Security audit (cargo-audit + npm audit) + hygiene |

Safety-critical changes (circuit_breaker, executor, token safety) ship with a 🛡️ safety:
CHANGELOG marker and rarely skip a minor bump — see `docs/core/versioning.md`.


---

## 17. Configuration Reference — Operator (config.yaml + CHIMERA_*)

### 17.1 Load order (`core/src/config.rs::AppConfig::load`)

Priority, highest → lowest (`AppConfig::load(path)`; documented in-source):

1. **Environment overrides**: `Environment::with_prefix("CHIMERA").separator("__")` — any
   `CHIMERA_SECTION__FIELD` env var overrides the matching config key
   (e.g. `CHIMERA_POSITION_SIZING__MIN_SIZE_SOL` → `position_sizing.min_size_sol`; lists are
   comma-separated via `list_separator(",")`). Env always wins; secrets only come via env.
2. **Custom config path** (CLI/programmatic argument, if provided).
3. `config/config.yaml` (deployment override directory).
4. `config.yaml` in CWD (the repo copy holds v1.0.0 defaults).
5. Struct defaults (`#[serde(default = ...)]` fns in `config.rs`).
6. Validation pass — startup fails on invalid combinations (fail-closed).


**Pure env vars** (read directly via `std::env::var`, never part of `config.yaml`; see
`api/src/main.rs`): the operator-internal knobs `CHIMERA_ENTRY_CONFIRMATION__{ENABLED,
WAIT_SECS,MAX_DRAWDOWN_PCT}`, all `CHIMERA_SELECTION__*` gate knobs (§6.2),
`CHIMERA_TRADE_MODE`, `CHIMERA_ENV`, `CHIMERA_PROFITABILITY_GATE__ENABLED` (bootstrap
override), `CHIMERA_JUPITER__DEVNET_SIMULATION_MODE`, `CHIMERA_JUPITER__API_KEY`,
`CHIMERA_SHADOW_MAX_LIFETIME_HOURS`, `CHIMERA_DB_MODE`, `CHIMERA_DB_PATH`, `CHIMERA_DEV_MODE`,
`CHIMERA_LOG_DIR`, `HELIUS_WEBHOOK_AUTH`, `HELIUS_API_KEY` (also injected into config); plus
`DATABASE_URL`, `REDIS_URL`, `RUST_LOG`, `DUNE_API_KEY`,
`TELEGRAM_BOT_TOKEN`/`TELEGRAM_CHAT_ID`, `TELEGRAM_API_ID/HASH` (scout collector), and
`RENT_SCAVENGER_{ENABLED,INTERVAL_SECS,BATCH_SIZE,MAX_RENT_LAMPORTS}`.
**Secrets that ARE `AppConfig` fields** (supply only via their env override, never in the
YAML file): `CHIMERA_SECURITY__WEBHOOK_SECRET` (required — startup refuses without it;
redacted from all logs/Debug output), `CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS` (rotation
overlap window), `CHIMERA_WALLET__PRIVATE_KEY` (or the encrypted vault, `infra/src/vault.rs`),
`CHIMERA_RPC__API_KEY`.

### 17.2 `AppConfig` sections (file value = repo `config.yaml`; prod = compose override)

**server** — `host` (0.0.0.0), `port` (8080), `request_timeout_ms` (30000).

**rpc** — `primary_provider` (helius), `primary_url` / `fallback_url` (prod: Helius RPC with
api-key), `rate_limit_per_second` (40), `timeout_ms` (2000), `max_consecutive_failures` (3),
`functional_health_check` (true — probes getLatestBlockhash, not just getHealth).

**database** — `path` (SQLite, dev only), `url` (PostgreSQL in prod; `CHIMERA_DB_MODE`
switches sqlite/postgres), `max_connections` (15).

**security** — `webhook_secret` (env-only), `max_timestamp_drift_secs` (60),
`webhook_rate_limit` (10 000), `webhook_burst_size` (15 000), `admin_wallets[]
{address, role}`, `api_keys[] {key, role}`.

**circuit_breakers** — `max_loss_24h_usd` (1000), `max_consecutive_losses` (7 → prod 10),
`max_drawdown_percent` (20 → prod 30), `portfolio_stop_loss_percent`, `cooldown_minutes` (30),
`max_jupiter_failures`.

**strategy** — `shield_percent` / `spear_percent` (file 50/50), `max_position_sol` (1.0),
`min_position_sol` (0.01), `shield_signal_quality_threshold` (0.40),
`spear_signal_quality_threshold` (0.40), `dex_fee_rate` (0.003),
`shield_max_total_cost_percent` (0.05 → prod 0.012), `spear_max_total_cost_percent`
(0.08 → prod 0.025), `slippage_fallback_small_percent` (0.005),
`slippage_fallback_large_percent` (0.01), `slippage_fallback_threshold_sol` (0.5),
`friction_gating_enabled` (→ prod false), `copy_wallet_sells` (true — false switches to
signal-trading mode where wallet SELLs are ignored).

**jito** — `enabled` (true), `tip_floor_sol` (0.0005), `tip_ceiling_sol` (0.003),
`tip_percentile` (50), `tip_percent_max` (0.03).

**jupiter** — `api_url` (`https://api.jup.ag/swap/v2`), `api_key` (env
`CHIMERA_JUPITER__API_KEY`; required in Live), `use_swap_v2` (true — meta-aggregator with
RTSE/Beam/gasless), `devnet_simulation_mode` (env; devnet only).

**queue** — `capacity` (1000), `load_shed_threshold_percent` (80).


### 17.3 Remaining `AppConfig` sections

**token_safety** (`CHIMERA_TOKEN_SAFETY__*`) — `freeze_authority_whitelist` /
`mint_authority_whitelist` (USDC, USDT, wSOL + DEX/pump program IDs allowed),
`min_liquidity_shield_usd` (25 000 file → prod 10 000), `min_liquidity_spear_usd`
(15 000 file → prod 10 000), `min_liquidity_pumpfun_usd`, `honeypot_detection_enabled` (true),
`cache_capacity` (1000), `cache_ttl_seconds` (3600), `liquidity_cache_ttl_secs` (60),
`fdv_cache_ttl_secs` (300), `liquidity_update_interval_secs` (30), `min_token_age_hours`
(24.0 file; unknown age = reject for Spear / warn for Shield; proven-waiver floor 6 min),
`min_token_age_pumpfun_hours`, `allow_unlisted_heuristic` (**false** — SECURITY CRITICAL:
when false, DexScreener-unindexed tokens count as $0 liquidity and are rejected; the true
heuristic assumes supply≈liquidity and is a honeypot attack vector), `allow_graduated_pumpfun`,
`allow_bonding_curve`, `max_bonding_curve_percent`, `denylist_enforce`,
`require_no_mint_authority`, `require_no_freeze_authority`.

**notifications** — `telegram.{enabled, bot_token(env TELEGRAM_BOT_TOKEN),
chat_id(env TELEGRAM_CHAT_ID), rate_limit_seconds}`; `rules.{circuit_breaker_triggered,
wallet_drained, position_exited, wallet_promoted, daily_summary, rpc_fallback}` (all true);
`daily_summary.{enabled, hour_utc (20), minute}`. Discord notifications also implemented in
`infra/src/notifications/`.

**monitoring** — `enabled`, `helius_api_key` (env), `helius_webhook_url` (public endpoint Helius
posts wallet events to), `webhook_registration_batch_size` (10) + `webhook_registration_delay_ms`
(200), `webhook_processing_rate_limit` (45), `rpc_polling_enabled` + `rpc_poll_interval_secs`
(8) + `rpc_poll_batch_size` (6) + `rpc_poll_rate_limit` (40), `max_active_wallets` (20),
`auto_demote_wallets` (true), `websocket_reconnect` (backoff params), `tiered_polling`
(conviction tiers + `conviction` thresholds), `inactivity_rotation`; **webhook_lifecycle**:
`auto_register_enabled`, `auto_cleanup_enabled`, `health_check_interval_secs` (3600),
`stale_threshold_days` (7), `max_registration_retries` (3), `helius_reconciliation_enabled`,
`helius_delete_orphaned`, `helius_dry_run`.

**profit_management** — `targets` [15, 40, 80, 150], `tiered_exit_percent` 50,
`trailing_stop_activation` 10, `trailing_stop_distance` 10, `hard_stop_loss` −8,
`time_exit_hours` 12, `losing_time_exit_hours_spear` 1, `wick_protection_secs` 30 (+
dust-min-exit to avoid leaving dust).

**exit_profile** — per-token learned exit parameters (activation/distance/target overrides by
hold-time cluster; enable + window knobs).

**position_sizing** — `base_size_sol` 0.25, `max_size_sol` 2.0, `min_size_sol` 0.05 (→ prod
0.10), `min_live_position_sol` 0.02, `consensus_multiplier` 2.0, `max_concurrent_positions` 8,
`use_kelly_sizing` true, `total_capital_sol` 10.0, `off_hours_size_multiplier` 0.5 (02:00–06:00
UTC), `conviction_size_multiplier` 1.5; capital-relative: `base_size_pct` (0.15),
`max_position_pct` (0.30), `shield_max_pct`, `spear_max_pct`, proven-boost size pct (0.15),
portfolio heat ceiling pct, `min_hold` sizing floor; WQS/confidence/hybrid weighting
(`hybrid_shield_wqs_weight`, `hybrid_spear_wqs_weight`).

**mev_protection** — `always_use_jito` (true), `exit_tip_sol` 0.002, `consensus_tip_sol` 0.001,
`standard_tip_sol` 0.0006.

**degradation** — performance-degradation detection thresholds driving auto-demotion + alerts.

**execution_lock** — idempotency lock TTL/capacity for double-fire protection.

**experiment** — `tracer_enabled`, `tracer_sample_rate` (1.0), `tracer_cap` (60),
`experiment_days` (21), `min_trades` (50), `controls_enabled`, `toxic_threshold_percent` (30),
`local_top_decline_pct` (8.0), `shakedown_mode` (false).

**profitability_gate** — `enabled` (prod true), `enforce_on_live` (false = live==paper policy),
`refresh_interval_seconds` (300), `inconclusive_size_factor` (0.5).

**rejection_mute** — mute wallets whose rejection rate exceeds thresholds (window + rate +
mute duration).

**shadow_blacklist** — auto-blacklist tokens whose shadow evidence stays negative (thresholds
+ expiry).

**dune** — Dune Analytics integration (`DUNE_API_KEY` env): endpoint, query IDs, cadence for
24 h wallet-PnL monitoring feeding auto-demotion; `api/src/bin/bootstrap_dune.rs` seeds
history.

**trade_mode** — `devnet | paper | dust_live | live` (default paper; prod env default paper).


### 17.4 Production env overrides applied by docker-compose (quick reference)

All values below are the operator container's env in `docker-compose.yml` (each carries an
in-file dated rationale). Format: `CHIMERA_SECTION__FIELD=value`.

| Env var | Prod value | Section ref |
|---|---|---|
| `CHIMERA_TRADE_MODE` | `paper` (`.env`-switchable to `dust_live`/`live`) | §5.2 |
| `CHIMERA_DB_MODE` / `CHIMERA_DB_PATH` | postgres mode | §17.2 database |
| `CHIMERA_SERVER__HOST` / `__PORT` | container host/port | server |
| `CHIMERA_RPC__PRIMARY_URL` / `__FALLBACK_URL` | Helius RPC (api-key) | rpc |
| `CHIMERA_JUPITER__API_KEY` | from `.env` | jupiter |
| `CHIMERA_CIRCUIT_BREAKERS__MAX_CONSECUTIVE_LOSSES` | 10 | §9.1 |
| `CHIMERA_CIRCUIT_BREAKERS__MAX_DRAWDOWN_PERCENT` | 30 (paper band) | §9.1 |
| `CHIMERA_POSITION_SIZING__MIN_SIZE_SOL` | 0.10 | §8.1 |
| `CHIMERA_STRATEGY__SHIELD_MAX_TOTAL_COST_PERCENT` | 0.012 | §17.2 strategy |
| `CHIMERA_STRATEGY__SPEAR_MAX_TOTAL_COST_PERCENT` | 0.025 | §17.2 strategy |
| `CHIMERA_STRATEGY__FRICTION_GATING_ENABLED` | false | §7.1 |
| `CHIMERA_TOKEN_SAFETY__MIN_LIQUIDITY_SHIELD_USD` / `_SPEAR_USD` | 10 000 each | §6.2 |
| `CHIMERA_PROFITABILITY_GATE__ENABLED` | true | §9.2 |
| `CHIMERA_ENTRY_CONFIRMATION__ENABLED/WAIT_SECS/MAX_DRAWDOWN_PCT` | true / 30 / 3.0 | §6.2 |
| `CHIMERA_SELECTION__REQUIRE_CONSENSUS_OR_PROVEN` | true (env-tunable) | §6.2 |
| `CHIMERA_SELECTION__MIN_PROVEN_TRADES` | 3 | §6.2 |
| `CHIMERA_SELECTION__REQUIRE_PROVEN_POSITIVE_PNL` | true | §6.2 |
| `CHIMERA_SELECTION__MIN_WQS_SCORE` | 70 | §6.2 |
| `CHIMERA_SELECTION__PROVEN_RECENCY_TRADES` | 5 | §6.2 |
| `CHIMERA_SELECTION__TOKEN_AGE_TRIAL_ENABLED` / `_MAX_SIZE_SOL` | true / 0.25 | §6.2 |
| `CHIMERA_SELECTION__SPEAR_LITE_MAX_SIZE_SOL` / `_WQS_THRESHOLD` | 0.25 / 40.0 | §6.2 |
| `CHIMERA_SELECTION__ENTRY_DRIFT_GUARD_ENABLED` / `MAX_ENTRY_DRIFT_PCT` | true / 1.0 | §6.2 |
| `CHIMERA_SELECTION__MIRROR_GATE_ENABLED` / `_MIN_AVG_PCT` / `_MIN_SAMPLES` / `_WINDOW_HOURS` | true / 1.5 / 3 / 72 | §6.2 |
| `CHIMERA_SELECTION__WALLET_TSTAT_ENABLED/_THRESHOLD/_MIN_SAMPLES/_WINDOW_DAYS` | on / 1.645 / n / d | §6.2 |
| `CHIMERA_SELECTION__WQS_TRIAL_ENABLED` / `_MIN_SCORE` | trial lane | §6.2 |
| `CHIMERA_SELECTION__PUMP_CHASE_ENABLED` / `_MAX_DELTA_PCT` | on / x | §6.2 |
| `CHIMERA_SELECTION__AVG_DOWN_ENABLED/_WINDOW_HOURS/_MIN_BUYS/_MIN_DROP_PCT` | on | §6.2 |
| `CHIMERA_SELECTION__SL_COOLDOWN_ENABLED/_HOURS/_LOSS_PCT` | on | §6.2 |
| `CHIMERA_SELECTION__TOKEN_VELOCITY_GATE_ENABLED/_MIN_LIQUIDITY_VELOCITY/_MAX_CURVE_COMPLETION` | on | §6.2 |
| `CHIMERA_SHADOW_MAX_LIFETIME_HOURS` | 24 | §6.2 |
| `CHIMERA_MONITORING__HELIUS_API_KEY` / `HELIUS_API_KEY` | from `.env` | monitoring |
| `CHIMERA_MONITORING__HELIUS_WEBHOOK_URL` | public URL Helius posts to | monitoring |
| `DUNE_API_KEY` | from `.env` | dune |

Note: `CHIMERA_SELECTION__*` and `CHIMERA_ENTRY_CONFIRMATION__*` are read directly by the
operator (not `AppConfig` fields) — they still follow the same naming convention.

---


## 18. Scout Configuration Reference (SCOUT_* env vars + CLI)

`scout/config.py` (`ScoutConfig`, ~1.6k lines) is the complete, single source of truth for
Scout configuration — every knob is a `SCOUT_*` env var with a documented default in that
file. This section groups them and records production values where they differ from defaults
(prod = docker-compose scout service env).

### 18.1 Env-var groups

| Group | Key vars (→ marks prod override) |
|---|---|
| Run control | `SCOUT_ENV`, `SCOUT_LOG_LEVEL` (→DEBUG), `SCOUT_VERBOSE` (→true), `SCOUT_MAX_WALLETS` (→250), `SCOUT_CONTINUOUS_INTERVAL`, `SCOUT_EMERGENCY_PAUSE` (0), `SCOUT_DATA_DIR`, `SCOUT_LOOKBACK_DAYS` |
| Discovery | `SCOUT_DISCOVERY_ENABLED` (→false — batch webhook discovery is used instead), `SCOUT_DISCOVERY_HOURS` (→72), fast/trending/deep windows (`SCOUT_DEEP_SCAN_HOURS` →168), `SCOUT_MULTI_TIMEFRAME_ENABLED/_GOAL/_PARALLEL`, `SCOUT_DISCOVERY_MIN_SOL` (→0.1), `SCOUT_MIN_WALLET_AGE_DAYS` (→7), `SCOUT_VALIDATE_WALLET_ACTIVITY` (→true), `SCOUT_ACTIVITY_RECENCY_HOURS` (→24), `SCOUT_ACTIVITY_VALIDATION_CONCURRENCY`, discovery limits (`_CONCURRENCY/_SEED_LIMIT/_LIMIT_PER_TOKEN/_PROGRAM_LIMIT/_BLOCK_LIMIT/_PROFITABILITY_FILTER/_FALLBACK_THRESHOLD/_CACHE_TTL/_TIMEOUT_SECONDS`), Sybil filters (`SCOUT_SYBIL_HOPS/_MULTIHOP_MAX`), `SCOUT_EXCHANGE_FUNDERS_PATH`, `SCOUT_DEDUP_TTL` |
| Wallet analysis | `SCOUT_WALLET_TX_LIMIT` (→60), `SCOUT_WALLET_TX_MAX_PAGES` (→15), `SCOUT_MIN_TRADE_COUNT` (→1), `SCOUT_MIN_CLOSES_REQUIRED`, `SCOUT_BALANCE_BATCH_SIZE/_FAIL_MODE`, `SCOUT_PARSE_HEALTH_EXIT_FAIL_PCT` (→25), `SCOUT_CROSS_WALLET_CORRELATION`, `SCOUT_CLUSTER_DEDUP/_ENSEMBLE` |
| WQS scoring | `SCOUT_WQS_MIN/_MAX`, `SCOUT_WQS_WEIGHT`, `SCOUT_WQS_RECENCY_WEIGHT`, `SCOUT_MOMENTUM_BOOST` (5.0), `SCOUT_SPLIT_WQS_ENABLED`, `SCOUT_WQS_BOOST_ENABLED`, `SCOUT_WQS_COMPARISON_MODE`, `SCOUT_MIN_WQS_ACTIVE` (→15.0), `SCOUT_MIN_WQS_CANDIDATE` (→10.0), `SCOUT_MIN_WQS_SWING/_WHALE`, `SCOUT_MIN_CONFIDENCE_ACTIVE` (→0.10), `SCOUT_MIN_CONFIDENCE_CANDIDATE`, `SCOUT_TIMING_WEIGHT/_MIN_SCORE`, `SCOUT_REGIME_WEIGHT`, `SCOUT_QUALITY_*` thresholds/window/sensitivity |
| Market regime | bull/bear/neutral/volatile multipliers (`SCOUT_*_REGIME_MULTIPLIER`), `SCOUT_ATR_PERIOD_DEFAULT/_THRESHOLD_PERIOD`, `SCOUT_REGIME_ATR_MULTIPLIER`, `SCOUT_REGIME_MODELS_ENABLED`, `SCOUT_MARKET_CONTEXT_FEATURES_ENABLED` |
| Promotion gates | `SCOUT_PROMOTION_MIN_TRADES` (→2), `SCOUT_MIN_CLOSE_RATIO{,_SCALPER,_SWING,_WHALE}` (→0.05 each), `SCOUT_FORBIDDEN_ARCHETYPES` (SNIPER), `SCOUT_MAX_REJECTION_RATE` (0.5), `SCOUT_MAX_PNL_REDUCTION_PCT` (80), `SCOUT_MAX_DRAWDOWN_FRACTION` (0.5), `SCOUT_MIN_AVG_HOLD_HOURS` (2.0), `SCOUT_ENFORCE_LOW_CHURN` (true), `SCOUT_FAST_TRACK_WQS_THRESHOLD` (80), `SCOUT_FAST_TRACK_MIN_TRADES` (5), `SCOUT_ARCHETYPE_DIVERSITY_MODE/_MIN_PCT`, `SCOUT_MIN_SCALPER/SWING/WHALE_COUNT`, `SCOUT_REVALIDATE_CANDIDATES` (→true, limit →20) |
| Walk-forward | `SCOUT_WALK_FORWARD_ENABLED` (true), `_HOLDOUT_FRACTION` (0.3), `_MIN_TRADES` (10), `_FALLBACK_PENALTY` (→2.0), `SCOUT_MIN_HOLDOUT_PNL_SOL` (0.01) |
| Backtest realism | `SCOUT_PRIORITY_FEE_SOL` (→0.0001), `SCOUT_JITO_TIP_SOL` (→0.003), `SCOUT_DEX_FEE_PCT`, `SCOUT_MEV_PENALTY_PCT`, `SCOUT_MAX_SLIPPAGE_PCT`, `SCOUT_ENTRY/EXIT_DELAY_SLIPPAGE_PCT`, `SCOUT_USE_CPMM_SLIPPAGE`, `SCOUT_USE_DYNAMIC_FEES`, `SCOUT_LIQUIDITY_MODE` (→simulated; `real` when Birdeye key set), `SCOUT_STRICT_HISTORICAL_LIQUIDITY` (→flexible), `SCOUT_ENFORCE_CURRENT_LIQUIDITY`, `SCOUT_HISTORICAL_LIQUIDITY_GRACE_PERIOD_DAYS`, `SCOUT_LIQUIDITY_ALLOW_FALLBACK`, `SCOUT_DEX_PROGRAM_IDS`, `SCOUT_TOKEN_2022_ALLOWLIST`, arbitrage-wallet filters (`SCOUT_ARB_MIN_TRADES_FOR_DETECTION/_ROUND_TRIP_THRESHOLD_PCT/_COOLDOWN_HOURS`) |


| Group (cont.) | Key vars |
|---|---|
| ML pipeline | `SCOUT_ML_ENABLED`, XGBoost (`SCOUT_XGBOOST_ENABLED/_LEARNING_RATE/_MAX_DEPTH/_N_ESTIMATORS/_SUBSAMPLE`), LightGBM (`SCOUT_LIGHTGBM_*`), `SCOUT_META_LEARNER_ENABLED`, `SCOUT_ENSEMBLE_ENABLED/_WEIGHT/_MIN_CONFIDENCE`, `SCOUT_MODEL_REGISTRY_ENABLED`, `SCOUT_MODEL_DIR`, `SCOUT_BATCH_INFERENCE_ENABLED/_SIZE`, `SCOUT_ONLINE_LEARNING_ENABLED`, `SCOUT_AUTO_RETRAIN_ENABLED`, `SCOUT_RETRAIN_INTERVAL_HOURS`, `SCOUT_MIN_SAMPLES_FOR_RETRAIN`, drift detection (`SCOUT_DRIFT_DETECTION_ENABLED`, `SCOUT_FEATURE/CONCEPT/ALERT_DRIFT_THRESHOLD`), `SCOUT_HYPEROPT_ENABLED/_TRIALS`, model compaction (`SCOUT_MODEL_PRUNING/QUANTIZATION/TORCH_ENABLED`), `SCOUT_SHAP_ENABLED`, MLflow (`SCOUT_MLFLOW_TRACKING_ENABLED/_URI`), `SCOUT_ML_LATENCY_BUDGET_MS` |
| Prediction validation | `SCOUT_PREDICTION_LOGGING/MATCHING/TRACKING_ENABLED`, `SCOUT_VALIDATION_ENABLED/_FORMAT/_SCHEDULE/_TIME_WINDOW(_DAYS)`, alert thresholds (`SCOUT_ALERT_HIGH_ERROR/LOW_ACCURACY/DRIFT_THRESHOLD`, `SCOUT_ALERT_WEBHOOK_URL/_DIR`), `SCOUT_ML/PRODUCTION_MONITORING_ENABLED`, `SCOUT_AB_TESTING_ENABLED/_TRAFFIC_SPLIT` |
| Feature store | `SCOUT_ADVANCED_RISK_FEATURES_ENABLED`, `SCOUT_NETWORK/TIME_SERIES_FEATURES_ENABLED`, `SCOUT_FEATURE_CACHE_ENABLED/_TTL_SECONDS`, `SCOUT_FEATURE_SELECTION_ENABLED`, `SCOUT_MAX_FEATURES`, `SCOUT_FEATURE_IMPORTANCE_THRESHOLD`, freshness weighting (`SCOUT_FRESHNESS_WEIGHT/_OPTIMAL/MAX_AGE_SECONDS`) |
| Signal quality | `SCOUT_SIGNAL_QUALITY_FILTER_ENABLED/_ADAPTIVE_THRESHOLD/_STATE_FILE`, `SCOUT_SIGNAL_FRESH/STALE/MAX_AGE_SECONDS`, `SCOUT_HIGH_CONVICTION_ENABLED`, `SCOUT_RISK_REWARD_RATIO_TARGET` |
| Stop-loss optimizer | enable + ATR multipliers per regime (`SCOUT_STOP_LOSS_*`), trailing config, `SCOUT_STOP_LOSS_MAX_RISK_PER_TRADE/_MIN_RISK_REWARD/_NOISE_TOLERANCE_PERCENT`, `SCOUT_MAX_TOTAL_RISK_PERCENT`, `SCOUT_MAX_CONCURRENT_EXITS` |
| Capital model | `SCOUT_STARTING/CURRENT/TARGET_CAPITAL`, `SCOUT_COPIER_SIZE_SOL`, `SCOUT_POSITION_SIZE_RISK_PERCENT/_MAX_PERCENT` |
| Caching | `SCOUT_CACHE_ENABLED/_TTL` (→3600), `SCOUT_LIQUIDITY_CACHE_TTL` (→60), advanced L1/L2/L3 (`SCOUT_CACHE_L1/L2/L3_ENABLED/_TTL_SECONDS`, `SCOUT_CACHE_MEMORY_MB/_WARMING/_AGGRESSIVE_EVICTION`), WQS-aware TTLs (`SCOUT_CACHE_EXCEPTIONAL/HIGH/AVERAGE/BELOW_AVERAGE_WQS_MULTIPLIER`, `SCOUT_CACHE_GROWTH_AWARE_TTL`, `SCOUT_CACHE_TTL_WALLET_METRICS/_HIGH_WQS_WALLET_DATA`), Redis L3 (`SCOUT_REDIS_ENABLED`/`SCOUT_REDIS_URL`), `SCOUT_DISCOVERY_CACHE_TTL`, `SCOUT_FRESHNESS_MAX_AGE_SECONDS` |
| Helius budget | `HELIUS_API_KEY`, `SCOUT_HELIUS_API_BASE_URL`, `SCOUT_CREDIT_TRACKING_ENABLED`, `SCOUT_MONTHLY_CREDITS/_TOTAL_ANALYSIS_CREDITS`, `SCOUT_MAX_API_CALLS_PER_RUN`, `SCOUT_MAX_REQUESTS_PER_SECOND/_TARGET_RPS`, adaptive rate limit (`SCOUT_RATE_LIMIT_ADAPTIVE/_MIN_DELAY_MS/_MAX_DELAY_MS`), `SCOUT_QUOTA_PROBE_INTERVAL_SECONDS`, Helius circuit breaker (`SCOUT_CIRCUIT_BREAKER_THRESHOLD/_RESET_SECONDS`), `SCOUT_BUDGET_TRACKING_ENABLED`, `SCOUT_MAX_WALLETS_TIER1/_TIER2`, percentile caps (`SCOUT_MAX/MIN_PERCENTILE_THRESHOLD`, `SCOUT_TOP_PERCENTILE_TARGET`) |
| Database/state | `DATABASE_URL` (psycopg3 pool), `SCOUT_DB_POOL_MAX_SIZE` (→30)/`_TIMEOUT` (→15), `SCOUT_STATE_PERSISTENCE_*` (enabled, DB path, backup + vacuum intervals, credit/ROI/wallet-performance history toggles, max days), `SCOUT_GROWTH_OPTIMIZED`, `SCOUT_PROFIT_TRACKER_ENABLED` |
| Safety | `SCOUT_SAFETY_FAIL_MODE` (RugCheck fail-mode), `SCOUT_EMERGENCY_PAUSE` |

Full literal list with defaults: grep `SCOUT_` in `scout/config.py` (~280 vars).

### 18.2 CLI (`python scout/main.py`)

| Flag | Default | Purpose |
|---|---|---|
| `--output/-o` | reporter DB-path ref | roster itself is written to PostgreSQL via `DATABASE_URL` |
| `--max-wallets` | 250 (`SCOUT_MAX_WALLETS`) | cap per run (200–500 on paid Helius) |
| `--verbose/-v` | off | verbose output |
| `--calibration-report` | off | print WQS calibration percentiles + suggested thresholds |
| `--dry-run` | off | analyze without DB writes |
| `--skip-backtest` | off | skip pre-promotion backtest (faster, less accurate) |
| `--walk-forward-min-trades` | 10 | min closed trades for walk-forward validation |
| `--min-wqs-active` / `--min-wqs-candidate` | env or defaults | promotion thresholds |
| `--min-liquidity-shield` / `--min-liquidity-spear` | 5000 / 2500 | liquidity prefilter |
| `--priority-fee-sol` / `--jito-tip-sol` | 0.0001 / 0.0001 | backtest cost model |
| `--clear-cache` | — | wipe all scout caches and exit |
| `--discovery-hours` | env default | discovery lookback window |
| `--wallet-tx-limit` / `--wallet-tx-max-pages` | env defaults | per-wallet fetch caps |
| `--continuous` + `--continuous-interval` | 300 s (prod: 7200) | loop mode |

Promotion criteria assembled from env (`_load_promotion_criteria`): `SCOUT_MIN_CONFIDENCE_ACTIVE`
(0.70 default; prod 0.10), `SCOUT_PROMOTION_MIN_TRADES`, close-ratio set, momentum, rejection
rate, PnL reduction, drawdown, walk-forward set, forbidden archetypes, fast-track pair — the
same vars as §18.1 promotion gates. `SCOUT_EMERGENCY_PAUSE=true` returns zero promotions.

### 18.3 HTTP-triggered runs

The operator exposes `POST /api/v1/scout/run` (dashboard "Run scout now") plus
status/metrics/budget/cache read endpoints; the scout container otherwise self-schedules via
`--continuous`.

---


## 19. Observability & Operations

### 19.1 Metrics & logs

- **Prometheus**: the API serves `GET /metrics` (prometheus crate). Custom series include
  signal admissions/rejections by gate reason, executor latency histograms, Jito tip history,
  circuit-breaker state, webhook health, rate-limit metrics. `ops/prometheus/` scrape config;
  Grafana dashboards in `ops/grafana/`; alert rules in `ops/alertmanager/`.
- **Structured logs**: Rust `tracing` (JSON-ish key=value), Python prefixed `print`
  (`[Scout] …`). Docker logging driver `json-file` with rotation (50 MB × 5).
- **Health**: `GET /api/v1/health` (liveness + component status), `GET /health/live`,
  `GET /health/ready` (compose healthchecks poll these), DB/Redis connectivity checks.
- **Decision audit**: `config_audit` table records every runtime config change (who/when/old
  value); `webhook_lifecycle_audit` records every webhook registration/removal; the
  decision recorder persists gate outcomes per signal for forensics.

### 19.2 Routine operator tasks

| Task | How |
|---|---|
| Tail prod logs | `make logs` / `make logs-all` (ssh journalctl/docker logs) |
| Check verdict/PnL | dashboard Performance page; `GET /api/v1/stats/performance`, `/risk/profitability/status` |
| Gate forensics | shadow-mirror stats endpoints + `decision_log`/shadow tables; `docs/paper-profitability*` investigations |
| Re-run scout now | dashboard button or `POST /api/v1/scout/run` |
| Emergency halt everything | set `SCOUT_EMERGENCY_PAUSE=1` (stops promotions) + `POST /api/v1/risk/circuit-breaker/trip` + kill-switch endpoints (flatten optional) |
| Revert a config experiment | `config_audit` shows prior value → PATCH again (defaults documented inline in `docker-compose.yml` comments — every override carries its REVERT criterion) |
| DB backup/restore | `make db-backup`, `ops/backup-postgres.sh` (WAL + nightly dumps), `make backup-verify`, `make rollback` |
| Wallet webhook reconciliation | `scripts/` batch registration script + `docs/runbooks/helius-webhook-missing.md` |

### 19.3 Alerting paths

Telegram + Discord notification adapters (`infra/src/notifications/`) fire on: circuit-breaker
trips/resets, kill-switch events, large losses, demotions, webhook coverage gaps, scout
promotion episodes, prediction-validation drift alerts (`SCOUT_ALERT_*`).

---

## 20. Testing & Versioning

### 20.1 Test matrix

| Suite | Command | What it covers |
|---|---|---|
| Rust unit + integration | `make test-operator` (`cargo test --workspace`; single test: `cargo test -p <crate> <name>`) | inline `#[cfg(test)]` modules + `operator/tests/`, `api/tests/` (execution, reconciliation, security, load) |
| Python unit | `make test-scout` (`pytest scout/tests/`, `-k` for single test) | WQS, backtester, validator, promotion, decimal utils; Hypothesis property tests for money math |
| Web | `make test-web` (Vitest) + Playwright E2E | API clients, components |
| Integration | `make test-integration` | API↔DB flows against ephemeral Postgres |
| Chaos | `make test-chaos` | outage/restart/resync behavior (RPC loss, DB blips, queue overflow) |
| Load | `make test-load` (k6) | webhook throughput vs rate-limiter |
| ML validation | `make validation` / `validation-match` / `validation-report` | predicted vs realized profitability (Pearson + t-stat per band), WQS calibration |
| Correlation backfill | `make backfill-correlation` | historical WQS↔copy-PnL correlation into `wqs_pnl_correlation` |
| Lint/format | `make lint` (clippy/ruff/tsc), `make fmt`; security `make audit` (cargo-audit + npm audit) |

CI gate mirrors: workspace tests + scout tests + web lint/tests on every push/PR.

### 20.2 Versioning & releases (`docs/core/versioning.md`)

- Unified SemVer across Rust/Python/web; **single source of truth = `VERSION`** at repo root.
- `make release TYPE=patch|minor|major` bumps `VERSION`, syncs `Cargo.toml`s,
  `web/package.json`, `scout/pyproject.toml`, docker labels; creates
  `chore(release): vX.Y.Z` commit + tag (`git push --follow-tags`).
- Never hand-edit component versions; `make version-check` fails the build on drift.
- Safety-critical changes (circuit breaker, executor, token safety) get a `🛡️ safety:`
  CHANGELOG marker; pre-releases use `--pre=alpha|beta|rc` and must never trade live.

### 20.3 Where to read more

| Topic | Doc |
|---|---|
| Product design (PDD) | `docs/core/pdd.md` |
| HTTP API reference | `docs/core/api.md` + `docs/core/openapi.yaml` |
| Gate-by-gate rationale | `docs/paper-profitability-2026-09.md`, `docs/wallet-selection-research.md`, inline comments in `docker-compose.yml` (every override documents its measurement + REVERT rule) |
| Runbooks | `docs/runbooks/` (webhook gaps, RPC failover, incident guide `ops/RUNBOOK-INCIDENTS.md`) |
| Subsystem conventions | `AGENTS.md` at root + `operator/`, `core/`, `infra/`, `api/`, `scout/`, `web/` |

---

*End of document. Keep this file updated when adding config options: the authoritative
sources remain `core/src/config.rs` (operator), `scout/config.py` (scout), and
`docker-compose.yml` (deployment wiring).*

