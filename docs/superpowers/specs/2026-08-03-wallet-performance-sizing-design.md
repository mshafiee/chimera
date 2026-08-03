# Design: Tiered Per-Wallet Copy-Performance Sizing

**Date:** 2026-08-03
**Status:** Approved (design) → implementation plan next
**Mode:** Paper trading only; feature default OFF (opt-in)

## Problem

Every admitted trade sizes at the flat **0.25 SOL floor** regardless of which
wallet generated the signal, even though copy-PnL varies hugely by wallet (e.g.
Grxr6mGL +0.069 vs 2Btg39je −0.031 over their samples). Capital is not
concentrated on proven edges.

A previous attempt to exploit wallet quality via the `signal_quality` score
failed: at current per-wallet sample sizes (3–16 trades) the quality bands are
non-monotonic noise and do **not** predict outcome. Any sizing-up must therefore
be gated on a **meaningful sample** and on **realized** (not predicted)
profitability, to avoid betting on variance.

## Goal

Allocate more capital to wallets with **proven recent copy profitability**, and
revert to the floor otherwise — without re-introducing the small-sample
overfitting that sank the quality-score approach.

## Design summary (tiered, conservative, recency-gated)

Two tiers only:

- **BASE (default):** 0.25 SOL (the existing hard floor). Unproven / small-sample
  / net-negative / dormant wallets.
- **BOOSTED:** **0.50 SOL**. Wallets that meet all "proven" criteria below.

Sizing only moves **up** (never below the floor). Bad wallets are already
removed by the existing churn-protection demotion, so this feature adds no new
"skip losers" path — it only up-weights proven winners.

### "Proven" criteria (ALL must hold → BOOSTED; else BASE)

| Gate | Threshold | Rationale |
|------|-----------|-----------|
| Sample size | ≥ **15** CLOSED copy trades in window | Above the noise floor where quality-score failed (n=5–15) |
| Window | last **20 trades** AND within **30 days** | Rolling, recent |
| Profitable | net PnL over window **> +0.01 SOL** | Trivially-small-positive excluded |
| Win rate | ≥ **40%** | Margin above the ~35–40% breakeven implied by typical avg-win/avg-loss |
| Recency | last copy trade within **7 days** | A proven-but-dormant wallet loses the boost (consistent with `auto_promote_max_age_days`) |

The tier is recomputed on every CLOSED copy trade and periodically, so a bad
stretch, a WR drop, or dormancy **auto-reverts** the wallet to BASE — the boost
self-revokes with no manual action.

## Data flow

1. `WalletPerformanceTracker::record_trade_result` already updates per-wallet
   metrics on each close (`copy_pnl_7d`, `winning_trades`, `total_trades`,
   `recent_trade_count`).
2. **New:** `compute_copy_tier(wallet) -> CopyTier { Base, Boosted }` derives the
   tier from the metrics. The metrics struct gains the fields needed to evaluate
   the gates (last-N-trades net PnL, last-N WR, last trade time). The existing
   7d PnL query is extended/paralleled to a "last 20 trades / 30d" query.
3. `selection.rs`, when building `SizingFactors`, reads the wallet's tier and
   attaches it (a `boost_target_sol: Option<Decimal>` field, `Some(0.50)` when
   Boosted).
4. `position_sizer::calculate_size`: when `boost_target_sol` is `Some`, the base
   size becomes that target (0.50), still subject to `strategy_max` and the
   final `min_size_sol` floor. Otherwise the existing 0.25 floor applies.

## Cost-gate fallback (pump.fun tokens are illiquid)

The execution cost gate (`executor.rs:3265`, `enforce_*_cost_limit`)
**hard-rejects** when `cost_pct > limit` — it does not cap. A 0.50 size has more
slippage than 0.25 and may be rejected on low-liquidity tokens, which would
silently neutralize the boost for exactly the tokens these wallets trade.

**Behavior:** for a BOOSTED trade, validate the cost gate at the boosted size
(0.50). On `ExecutionCostTooHigh`, **fall back to the floor (0.25)** and
re-validate; if the floor also fails, reject as today. So a proven wallet trades
at 0.50 when liquidity allows, else at 0.25 — never skipped solely because the
boost was too big.

Implementation note (to finalize in the plan): the executor must know a trade
was boosted + its floor, so it can retry on cost failure. The boost target and
the floor are both already known at execution time (`position_sizing` config), so
this is a localized retry in the cost-check path, not a pipeline-wide change.

## Configuration (new `monitoring.wallet_boost_*`, default OFF)

```
wallet_boost_enabled: false           # opt-in
wallet_boost_min_sample: 15
wallet_boost_window_trades: 20
wallet_boost_window_days: 30
wallet_boost_min_net_sol: 0.01
wallet_boost_min_winrate: 0.40
wallet_boost_recency_days: 7
wallet_boost_size_sol: 0.50           # the BOOSTED target size (hard cap)
```

All numeric knobs live in config so they can be tuned without code changes once
more per-wallet samples accumulate.

## Safety

- **Hard cap 0.50** (2× floor). No wallet can exceed it.
- **Portfolio heat** (2.0 SOL cap) naturally limits how many boosted positions
  run concurrently (a 0.50 position consumes 2× a floor position's heat).
- **Cost gate** backstops slippage on the larger size (with the floor fallback).
- **Default OFF** — zero behavior change unless enabled. All existing sizing,
  demotion, and recency logic is untouched.
- The boost **self-revokes** (any gate failure → BASE), so a wallet cannot get
  "stuck" boosted after going bad.

## Testing

- **Unit — tier classification:** each gate boundary (sample < 15, WR < 40%,
  net ≤ 0.01, dormant > 7d → BASE; all-pass → BOOSTED). Property-style: a wallet
  that meets the bar, then loses 5 in a row / goes dormant, reverts to BASE.
- **Unit — sizer:** `calculate_size` returns 0.50 when `boost_target_sol=Some`,
  0.25 when `None`; boost respects `strategy_max`; final floor still applied.
- **Unit — cost fallback:** a BOOSTED trade whose 0.50 cost exceeds the limit
  falls back to 0.25 and proceeds; if 0.25 also fails, rejects.
- **Integration:** proven wallet's signals size 0.50; same wallet after a
  dormancy window sizes 0.25.

## Out of scope (explicitly deferred)

- Continuous multiplier / fractional-Kelly per wallet (Approach B/C) — revisit
  once per-wallet samples reach 50+ and edges are statistically stable.
- Per-strategy tiering — the tier reflects the wallet's overall copy edge and is
  strategy-agnostic for v1.
- Down-sizing losers below the floor — impossible by design (0.25 hard floor);
  handled by demotion.

## Open integration detail (resolved during planning)

Exact placement of the cost-gate floor fallback (retry loop in
`enforce_*_cost_limit` vs a wrapper at the executor call site) and how
`boost_target_sol` threads from `SizingFactors` → `calculate_size` → executor —
to be specified precisely in the implementation plan against the current code.
