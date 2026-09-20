# Chimera: Complete System Report (2026-09-20)

**Scope:** what the system does, how it is built, what market it trades, and —
with live production evidence — why it cannot be profitable in its current form.

**Evidence base:** all production numbers below were collected read-only from
`chimera-01.moez.tech` (`/opt/chimera`) on 2026-09-19/20. Reproduction queries
are in the Appendix. Analysis artifacts:
`docs/superpowers/analysis/2026-09-19-realized-cohort-mining.md`,
`docs/superpowers/plans/2026-09-19-profitability-search-program.md`,
`docs/runbooks/2026-09-07-shadow-validation-protocol.md`,
`docs/runbooks/2026-09-19-mirror-robustness-protocol.md`.

---

## 1. What this system does

Chimera is a **high-frequency copy-trading platform for Solana memecoins**.
Its core loop:

1. **Watch** a roster of tracked ("smart money") wallets via Helius webhooks.
2. When a tracked wallet buys a token, **evaluate** the signal through a gate
   chain (wallet status, WQS score, token safety, liquidity, age, shadow-mirror
   history, signal quality, drift, size).
3. **Size** the copy (base × WQS × confidence × boost/penalty multipliers,
   clamped to min/max).
4. **Execute** — in paper mode record a simulated trade; in live mode route a
   real swap through Jupiter (optionally bundled via Jito).
5. **Manage the exit** through profit targets, trailing stops, stop-losses,
   time exits, and wallet-sell mirroring.
6. **Record everything** — every signal produces an immutable `decision_records`
   row; every signal (admitted or rejected) produces counterfactual
   `shadow_positions`/`shadow_exits` rows so rejected decisions can be scored
   after the fact.

Three operating modes (`docs/trading-modes.md`): **paper** (default, simulated,
no capital at risk), **dust-size live** (tiny real trades to measure fills),
**full live** (real capital; gated behind a profitability verdict that has
never passed).

A parallel **shadow trader** mirrors every rejected signal with several exit
strategies (`mirror_main`, `wallet_sell`, `fixed_1h/4h/24h`, …) so the question
"what if we had admitted this?" is answerable from data, not backtest fiction.

---

## 2. Architecture

```
                    ┌──────────── Helius ────────────┐
                    │ webhooks · RPC · polling       │
                    └───────┬────────────────┬───────┘
                            │ swaps          │ prices
              ┌─────────────▼────────────────▼─────────────┐
              │  OPERATOR (Rust hot path, chimera_operator) │
              │  webhook → selection → sizer → executor    │
              │  exits · circuit breakers · shadow trader  │
              └───────┬────────────────┬──────────────────┘
                      │ trades         │ decisions/shadow
              ┌───────▼────────────────▼────────┐  ┌──────────────────┐
              │  PostgreSQL (source of truth)   │  │ SCOUT (Python    │
              │  trades · decisions · shadow ·  │  │ cold path)       │
              │  wallets · verdicts             │  │ Helius history → │
              └───────┬────────────────────────┘  │ WQS → backtest → │
                      │                           │ promote/demote   │
              ┌───────▼────────┐                  └──────────────────┘
              │ API (Rust) +   │
              │ WEB (React)    │
              │ dashboard      │
              └────────────────┘
  External: Jupiter (quotes/swaps) · Jito (bundles) · Dune (wallet PnL) ·
            GeckoTerminal (prices) · Telegram/Discord (alerts)
```

| Component | Stack | Role |
|---|---|---|
| `operator/` | Rust (`chimera_operator`) | Signal → selection → sizing → execution → exits; circuit breakers; shadow trader; verdict gates |
| `core/` | Rust (`chimera_core`) | Shared config, models, price cache, slippage/MEV math, experiment/tracer machinery |
| `infra/` | Rust (`chimera_infra`) | Postgres abstraction, Helius/Jupiter clients, token-safety (honeypot, liquidity, bonding curve), vault, notifications |
| `api/` | Rust (`chimera_api`) | Dashboard backend: wallets, trades, profitability verdict, monitoring, auth |
| `scout/` | Python 3.11 | Cold path: Helius history → Wallet Quality Score → backtest → promotion/demotion; cluster detection; validation instruments |
| `web/` | TypeScript/React 18 + Vite | Operator dashboard |
| `migrations/` | SQL (`sqlx`) | Postgres schema incl. decision recorder, shadow trader, promotion episodes, cluster triggers |

**Key design decisions (and their consequences):**

- **Immutable decision log.** Every signal is recorded with its gate outcome
  (`decision_records`), so the rejection funnel is measurable. This is the
  system's best feature — and the instrument that convicted it.
- **Shadow counterfactuals.** Every rejected signal gets simulated exits, so
  "loosen gate X" is a data question. Correct in principle; in practice the
  shadow marks proved systematically optimistic (Section 5.3).
- **Pre-registered verdict gates** (`docs/profitability-gates.md`): 8 gates
  (sample ≥60, net-return 95% CI > 0, all-cohort positivity, bias ≤5%,
  single-loss/drawdown caps, zero missing outcomes, ≥99% completeness).
  Live trading is fail-closed until GO. Verdict on 2026-09-19: **STOP**.
- **Fail-closed risk controls.** Circuit breakers (consecutive losses,
  drawdown, Jupiter failures), token-safety (honeypot simulation, liquidity
  floors, pump.fun bonding-curve handling), cost-efficiency gating. These work
  as designed — they correctly reject bad flow, which is why the book is small
  rather than catastrophic.
- **Single-provider dependency.** RPC primary *and* fallback default to the
  same Helius API key ("one quota event halts all trading" — the compose file
  says so itself). Helius 429 on 2026-09-20 halted *all* ingestion and pricing.

---

## 3. The nature of the market it trades

Chimera copies wallets trading **Solana memecoins**, mostly hours-old tokens
from pump.fun. This market has a specific microstructure, and every property
of it works against a copy-trader:

1. **You buy after the informed party moved the price.** Copying means entering
   *after* the whale's buy has already pushed the price up, and exiting after
   (or before) the whale sells. The 2026-08-21 finding measured a 3.4-minute
   mean whale→fill lag with 14% of entries >5% above the whale. Adverse
   selection is not a risk here — it is the entry mechanism.
2. **Payoffs are lottery-shaped.** The shadow book's own shape: median exit
   0.00%, ~10% win rates, and a mean carried by a handful of +1,000%–+7,960%
   moonshots. Bases are frequent small losses punctuated by rare violent
   pumps. Any mean-based statistic (and any gate tuned on means) is dominated
   by a few unrepeatable, usually unfillable, outliers.
3. **Liquidity is the exit, not a number.** A token can show $50k "liquidity"
   while a copy-sized sell moves the price 10–50% or fails entirely. The shadow
   trader marks exits at mid-prices that assume fills; live `wallet_sell`
   exits (7d: 9.6% win, +1.21% avg) vs `mirror_main` marks on identical signals
   show the gap. 79% of the dune-cohort mirror sample was `no_price` — tokens
   that could not be priced at all, i.e. could never have been exited.
4. **Speed is a moat you don't have.** The profitable side of memecoin flow is
   being first (sniper bots, bundle priority, Jito tips). A webhook-driven
   copier pays the loser's side of every spread: entry slippage, Jito tips
   (0.0015–0.007 SOL), 0.3% DEX fees — roughly 0.6–1.5% round-trip before any
   edge, against a book averaging −1.86%/trade.
5. **Regimes decay in weeks.** `mirror_main` on identical logic fell from
   +20.9% avg (early Sep) to +3.40% (mid Sep) with win rate 38.8% → 19.6%.
   Edges found in this market are consumed by competition almost immediately;
   anything tuned on last month's marks is stale on arrival.
6. **The data itself is adversarial.** Honeypots (unsellable tokens),
   wash-traded volume, spoofed liquidity, dead pools — the token-safety stack
   exists because most signals are traps. The system correctly rejects ~99.6%
   of signals; the question is whether the 0.4% admitted contains any edge.
   Measured answer: no.

---

## 4. The evidence (production, 2026-09-19/20)

| Measurement | Value |
|---|---|
| Profitability verdict | **STOP** — net_return mean **−1.86%/trade**, 95% CI −3.42%…−0.30%; 0/2 cohorts positive; 143 missing outcomes |
| Closed book | 261 BUYs, **−2.078 SOL**; SHIELD 25% win, SPEAR 31% |
| Realized cohorts (WQS × size, per-SOL net) | <60&<1.0: −3.26% (t=−5.90); ≥60&≥1.0: −2.40% (t=−2.24); **every n≥10 bucket negative**; best candidate t=0.57 (noise) |
| Liquidity / hold-time / size / WQS bands | **All negative**; no band, wallet (16/17 lose), or duration is positive |
| Shadow twins (30d, `mirror_main`) | Admitted 145: 42% win, **−0.64% avg** vs rejected 11,660: 36% win, +10.88% raw — **the admitted cohort is the worst cohort** |
| Frozen Pivot-A verdict (14d, 09-06→09-20) | Legacy bar **GO** (n=2,063, mean +8.69, CI-lo +1.44) — **mechanical only** |
| Robustness gate, same window | **FAIL**: priced n=489, winsorized mean **−0.15**, median **−8.43**, cost-adjusted **−2.15**; live rail `wallet_sell` winsorized **−3.92** (CI −5.97…−1.77) |
| Cluster hypothesis | **INCONCLUSIVE** — n=36 triggers, zero after 09-10; capture alive, roster too small to form clusters |
| Conversion | 235 admitted → 80 closed (34%); **670 DEAD_LETTER vs 266 CLOSED** |
| Consensus | 2,562/2,563 admitted trades have `consensus_wallet_count=1` — multi-wallet consensus never fires |
| Roster (09-20) | 3 ACTIVE (repaired 09-20), 37 PROVING, 10,940 CANDIDATE; 92% of flow from one WQS-10 lottery wallet |
| Infra | Helius 429 on all endpoints (quota exhausted); CB latched on stale 15.3% drawdown; Dune promote queries failing hourly (billing); 56k Jupiter 0-price tombstones/day |

---

## 5. Why this system can't be profitable

Not one cause — **five independent, each sufficient**, all measured:

### 5.1 Selection is inverted at the margin (the deepest problem)
The engine's admitted cohort loses −0.64% on shadow twins and −1.86%/trade
realized, while the surrounding rejected pool shows positive shadow
expectancy. WQS measures *the whale's own PnL*; copy-PnL with *our exits* is a
different objective, and the two diverge (the code comments admit the two best
copy-targets sit at WQS 10 while WQS-80 wallets are dedup-negative). The trial
lane already admits sub-floor wallets at micro size — and the book still loses.
There is no gate setting that fixes a scorer pointed at the wrong target.

### 5.2 Shadow marks are not fills, and every "edge" lives in the gap
All three positive-looking numbers in the system's history (WQS_TOO_LOW +47.9
SOL, TOKEN_TOO_NEW +1,394 SOL, Pivot-A +8.69 mean) collapse under the same
autopsy: moonshot marks on illiquid tokens, `no_price` dead rows padding `n`,
win rates of 6–10%, trimmed means negative. The live-reachable rail
(`wallet_sell`) is negative everywhere it is measured. **No selection input to
the admission decision is a measured fill** — and the one fill-truth
instrument (dust-size tracer) was never funded.

### 5.3 The exit rail ≠ the calibration rail
Selection was tuned against `mirror_main` marks, but live positions exit
through profit-management targets that behave like the *worst* shadow
strategies. The 0.5h average hold, 2,459 adaptive-stop overrides/48h, and
−9.75% average shadow loser say the live rail banks the left tail while the
shadow rail that justified the trade shows the right tail.

### 5.4 Costs exceed the available edge by construction
~0.6–1.5% round-trip (Jito + DEX + slippage) against a book averaging −1.86%.
Gross PnL is negative *before* costs (−1.09 SOL/30d SHIELD gross), so fee
optimization — the classic fix — is arithmetically irrelevant here.

### 5.5 The system is starved of the only thing that could save it: flow data
Roster collapse (5→3 ACTIVE, none ever traded) plus Helius quota exhaustion
(429 on every endpoint) plus a latched circuit breaker means zero ingestion,
zero prices, zero fills. Even a correct hypothesis cannot accumulate evidence.
And the death spiral is structural: polling fallbacks burn the same single
Helius quota that webhooks, RPC, and prices share.

**Honest summary:** the strategy has negative expectancy per unit of capital in
every measurable cohort (−1.4% to −3.3%/SOL); the measurement apparatus that
could find an edge was itself broken in four places (all repaired 2026-09-19);
and the infrastructure that would carry new evidence is quota-dead. Tuning
cannot help because there is nothing to tune *toward* — no positive cohort
exists in the data.

---

## 6. What would have to change (and what was done)

Done in this session (all on `main`, deployed where applicable):

- Repaired roster to match reality (demoted decorative ACTIVE, promoted
  flow wallets to a paper measurement lane; audit-logged).
- Widened the paper drawdown band 15→30 so the latch can clear and the
  robustness window can accumulate.
- Repaired the verdict instrument four times over (it could never have run)
  and froze a moonshot-robustness gate for the next window (09-20→10-04).

Still required, all external to engineering:

1. **Helius quota** (account/plan action) — without it, nothing ingests.
2. **Dune billing** — without it, the proven-waiver oracle stays dead.
3. A robustness-gate **GO on fills** (or dust-live fill validation with real,
   small capital) before *any* live sizing — the current GO is marks-only.
4. If the 10-04 robustness verdict fails: **park the system** per the
   pre-registered stop criteria, rather than tuning gates further.

## Appendix — reproduction (read-only, on `chimera-01` in `/opt/chimera`)

```sql
-- Verdict inputs
SELECT ... -- GET /api/v1/profitability/verdict (live)
-- Book baseline
SELECT count(*), AVG(net_pnl_sol), SUM(net_pnl_sol) FROM trades
WHERE side='BUY' AND status='CLOSED' AND pnl_data_valid;
-- Decisive decomposition (§5)
SELECT CASE WHEN dr.wqs<60 THEN '<60' ELSE '>=60' END wqs,
       CASE WHEN t.amount_sol<1.0 THEN '<1.0' ELSE '>=1.0' END size,
       count(*), AVG(t.net_pnl_sol), SUM(t.net_pnl_sol), SUM(t.amount_sol)
FROM trades t JOIN decision_records dr ON dr.trade_uuid=t.trade_uuid
WHERE t.side='BUY' AND t.status='CLOSED' AND t.pnl_data_valid GROUP BY 1,2;
-- Twins
SELECT main_admitted, count(*), AVG(pnl_pct) FROM shadow_comparison
WHERE exit_strategy='mirror_main' GROUP BY 1;
-- Funnels
bash scripts/profitability_loop.sh
-- Frozen + robustness verdicts (scout container)
docker exec chimera-scout sh -c \
 'cd /app && python -m scripts.cluster_revalidation --mirror-validation 14 --json'
docker exec chimera-scout sh -c \
 'cd /app && python -m scripts.cluster_revalidation --robustness-validation 14 --json'
```
