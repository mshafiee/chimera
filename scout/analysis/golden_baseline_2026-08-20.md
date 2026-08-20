# Copy-engine backtest — golden baseline (Phase 1)

Generated 2026-08-20 from production shadow history via `scout/core/copy_backtest.py`
(committed baseline for future before/after diffs of any filter/exit change).

Method: `shadow_positions` + `shadow_exits` + `trades` (PostgreSQL). Cost-adjusted:
per-position cost = observed `SUM(total_cost_sol)/SUM(amount_sol)` from closed
`trades` applied pro-rata to shadow notional (≈1.5% per 1 SOL position). `sum_pnl`
is in SOL (1 SOL notional per position); `mean/median/...` are statistical values.

## Realize-vs-price gap (Phase 2 headline)
- Predicted win rate (`shadow_exits` mirror_main): **62.4%** (n=16,387)
- Realized win rate (closed copy `trades`): **18.0%** (n=183)
→ The documented realize-vs-price gap is confirmed and large on production data.

## Per-exit-strategy (cost-adjusted)
| strategy | n | mean | median | win% | sum_pnl |
|----------|-----|-------|--------|------|---------|
| dune_wallet | 3,000 | +0.0507 | −0.0015 | 45.9% | **+152.2** |
| **mirror_main** | 16,387 | +0.0052 | −0.0149 | 13.0% | **+85.0** |
| fixed_1h | 16,265 | +0.0037 | −0.0149 | 8.4% | +59.7 |
| fixed_24h | 15,949 | −0.0281 | −0.0150 | 10.0% | −447.9 |
| fixed_4h | 16,187 | −0.0319 | −0.0149 | 8.2% | −515.6 |
| wallet_sell | 16,428 | −0.0211 | −0.0149 | 7.7% | −347.3 |

Cost-adjusted medians sit at ≈ −1.5% for every strategy (the per-position cost
floor): per-trade edge is small vs cost, so profitability is driven by the
right tail (mirror_main/dune/fixed_1h) vs long/whale-exit bleeding
(fixed_24h/fixed_4h/wallet_sell). **Keep mirror_main; do not adopt
fixed_4h / wallet_sell / fixed_24h.**

## Per-gate under mirror_main (cost-adjusted; positive sum = gate rejected that would-have-been-profitable)
| gate | n | mean | median | win% | sum_pnl | note |
|------|-----|-------|--------|------|---------|------|
| TOKEN_TOO_NEW | 1,232 | +0.183 | −0.037 | 40.3% | +225.1 | fat tail (sd 2.81) — NOT robust; don't loosen |
| WQS_TOO_LOW | 482 | +0.083 | −0.013 | 43.4% | +40.0 | mildly over-strict (candidate) |
| PUMPFUN_INSUFF_LIQ | 99 | +0.016 | −0.037 | 43.4% | +1.6 | weak n |
| WHALE_AVERAGING_DOWN | 5 | +0.082 | +0.043 | 60.0% | +0.4 | n too small |
| ADMITTED | 188 | −0.017 | −0.035 | 31.9% | **−3.1** | engine's own cohort net-negative |
| SIGNAL_QUALITY_TOO_LOW | 525 | −0.022 | −0.065 | 34.5% | −11.3 | protective |
| PUMPFUN_BONDING_CURVE | 275 | −0.054 | −0.045 | 21.8% | −14.9 | protective |
| WALLET_MUTED | 5,841 | −0.014 | −0.015 | 2.5% | −80.5 | approx cost-floor; noise filter |

Remaining gates (NON_SPECULATIVE, TOXIC, LIQUIDITY_BELOW_MIN, SINGLE_WALLET_UNPROVEN,
SHADOW_MIRROR_*, STOP_LOSS_COOLDOWN, ALREADY_PUMPED, PUMP_CHASE) are neutral-to-negative =
protective/neutral; keep.

## By-strategy (mirror_main, cost-adjusted)
- SHIELD (n=16,305): +87.3, win 12.9%
- SPEAR (n=82): −2.3, win 26.8%

## Caveats
- Cost model uses a flat observed cost-per-SOL (~1.5%). This can be inflated by
  small positions where costs dominate; a position-size-conditional cost model
  is a candidate refinement before acting on magnitude.
- `shadow_exits` stores only the 5 fixed exit-strategy snapshots (not full price
  paths), so arbitrary parameter grid-search (e.g. different stop floors / defer
  ticks) cannot be replayed from this table alone — the harness measures the
  recorded strategies and the realize-vs-price gap.
