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

## Realized fill-skew (added 2026-08-20) — why the gap CANNOT be tuned yet
`CopyBacktest.fill_skew_report()` over closed SELL trades:
- **n = 4** real closed sells; realized live-vs-mark fill skew median ≈ **0.028%**
  (max 0.045%). Negligible sample.
- The copy engine has barely traded: **183 closed trades ever, 4 recent sells.**
  The 62.4% predicted win rate comes from **16,391 shadow simulations**, not
  realized fills.
- **Conclusion:** the realize-vs-price gap (62.4% predicted vs 18.0% realized) is
  real but the *realized* side is statistically meaningless (n≈183 / n=4 sells).
  Tuning `smart_exit::should_defer_exit` (`skew_pct`, `defer_max_ticks`) against
  4 fills is not data-driven — it is fabrication.
- **Prerequisite to ever close the gap:** (1) let the engine trade at real
  throughput so realized closes accumulate past a meaningful n, and (2) record a
  per-position price-mark series going forward (the DB has no price history; only
  entry/exit snapshots and `price_at_signal`). With both collected, re-run
  `fill_skew_report` and a deferral grid-search.

## Phase 2 replay implementation (2026-08-20) — gap now tunable via paths
Built the price-path + replay pipeline and validated it end-to-end on production:
- **Source discovery:** Helius swap reconstruction is blocked on this plan — the
  address-activity feed (`/addresses/{token}/transactions`) returns no
  `tokenTransfers`/`events.swap`, and the `/v0/transactions` batch-parse returns
  "Method not found". **GeckoTerminal public OHLCV** (no key) is the available
  price-path source for tokens with a live pool (`scout/core/price_path.py`:
  `geckoterminal_ohlcv` + `parse_ohlcv_close`). Helius swap extraction remains as
  best-effort if an enriched endpoint becomes available.
- **Persistence:** `price_path_points` + `price_path_fidelity` tables (migration
  `0020_price_path_replay.sql`, applied on prod); `scout/core/fidelity.py` computes
  path-vs-provider Pearson/MAPE/gate so a grid-search only trusts passing paths.
- **Trustworthy replay:** `operator/src/engine/exit_rules.rs` is now the single
  source of truth for the exit decision; `shadow_trader::check_mirror_main`
  delegates to it. `operator/src/bin/replay_exit.rs` replays reconstructed paths
  through the real rules with `ProfitManagementConfig` overrides.
- **End-to-end validation:** `path --limit 1` reconstructed 1 token / 25 hourly
  points from GeckoTerminal and persisted them; the Rust replay binary ran the
  position's path through the real exit rules and exited `recovery_gate` at
  −7.69% (~95 min hold). Replay entry is anchored to the first in-window path
  price (shadow positions often lack a recorded USD entry price).
- **CLI:** `analysis.copy_backtest_cli path|replay` orchestrate reconstruction,
  persistence, and replay (Rust binary via `--binary REPLAY_EXIT_BIN`).
- Next: run a full (or quota-budgeted) reconstruction over shadow tokens, apply
  the fidelity gate, then grid-search `ProfitManagementConfig`/smart-exit params
  and diff each candidate set against this baseline.


