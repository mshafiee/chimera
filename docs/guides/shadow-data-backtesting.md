# Shadow-Data Backtesting Guide

**Last Updated:** 2026-08-21

---

## Overview

This guide documents how to backtest the copy-engine's **entry filters** and
**exit (closing) algorithms** on shadow trading history and how to make
data-driven optimization decisions from the results. It covers the exact
commands, the data each report reads, how to interpret the numbers, and the
guardrails that keep every shipped change data-justified.

The tooling is the result of Phases 0-2H (see `scout/analysis/golden_baseline_2026-08-20.md`
for the committed baseline). Core principle: **no decision ships without a
repeatable, cost-adjusted, sufficiently-sampled backtest behind it.**

All commands run inside the **scout container** on the production server:

```bash
ssh root@chimera-01.moez.tech
docker exec -i chimera-scout sh -c 'cd /app && python -m ...'
```

---

## What the reports measure

| Command | Report | Reads from | Measures |
|---|---|---|---|
| `copy_backtest_cli exit` | per-exit-strategy | `shadow_exits`, `shadow_positions` | cost-adjusted PnL for each closing algorithm (mirror_main, fixed_1h/4h/24h, wallet_sell, dune_wallet) |
| `copy_backtest_cli gate` | per-gate under mirror_main | `shadow_exits` + rejection codes | cost-adjusted PnL of each entry filter bucket (ADMITTED vs every rejection reason) |
| `copy_backtest_cli strategy` | by-strategy | `shadow_exits` | SHIELD vs SPEAR split |
| `copy_backtest_cli gap` | predicted vs realized | `shadow_exits` + `trades` | predicted win rate (shadow mirror_main) vs realized win rate (closed trades) |
| `copy_backtest_cli skew` | fill skew | `trades` (recent SELL closes) | realized live-vs-mark gap + defer-trigger bands |
| `copy_backtest_cli mark` | recorded marks | `position_price_marks` (migration 0021) | per-position recorded price-mark geometry (drawdown, dip recovery) |
| `copy_backtest_cli reconcile` | per-trade shadow vs realized | `positions` + `shadow_exits` | the price-basis gap between shadow mirror_main and what actually happened |
| `copy_backtest_cli screen` | post-cost entry screen | `positions` + `shadow_exits` | per-wallet NET (post-cost) expectancy, verdicts CLEAR/MARGINAL/NEGATIVE |
| `exit_grid_search.py` | grid-search | `price_path_points` (source=shadow) or `position_price_marks` (source=real) | exit-parameter sensitivity through the real `replay_exit` rules |

---

## Workflow

### 1. Baseline the closing algorithms

```bash
docker exec -i chimera-scout sh -c 'cd /app && python -m analysis.copy_backtest_cli exit'
```

Interpret (current reference, 2026-08-21): only **mirror_main** (+97.5 SOL,
n=16,554) and **dune_wallet** (+152.4, n=3,000) are cost-adjusted positive.
`fixed_4h` (-515), `fixed_24h` (-454), `wallet_sell` (-354), `fixed_1h` (+66)
are not. **Policy: keep mirror_main; drop the fixed/wallet-sell variants.**

### 2. Baseline the entry filters

```bash
docker exec -i chimera-scout sh -c 'cd /app && python -m analysis.copy_backtest_cli gate'
```

Interpret: the engine's **ADMITTED** cohort is net-negative (-2.99, n=188) —
what it copies loses. `TOKEN_TOO_NEW` (+226) is **tail-only** (median -3.6%,
sd 2.8) — **do not loosen**. `WQS_TOO_LOW` (+40, sd 0.31, median -1.2%) is the
only mildly-robust positive bucket, still below cost at the median — treat as
a controlled-experiment candidate only. Nearly every gate's median sits at
**~ -1.4% (the round-trip cost floor)**; all shadow "edge" above it is
tail-driven. **Policy: no gate constant change ships from this report alone.**

### 3. Check the predicted-vs-realized gap

```bash
docker exec -i chimera-scout sh -c 'cd /app && python -m analysis.copy_backtest_cli gap'
docker exec -i chimera-scout sh -c 'cd /app && python -m analysis.copy_backtest_cli skew'
```

The headline gap (62.6% predicted, n=16,554 vs 18.8% realized, n=186) is
expected and **explained**, not a bug to chase: `reconcile` shows the shadow
price basis tracks reality (median gross gap 0.15%, n=84) and the entire net
drag equals the ~1.5% cost floor (net gap 1.49% vs cost 1.49%, corr 0.08).
`skew` at n=4 is statistically meaningless — **never tune deferral on it**.

### 4. Optimize only with sufficient, post-cost data

The two instruments that justify changes:

```bash
# per-trade shadow vs realized: is the shadow basis still faithful?
python -m analysis.copy_backtest_cli reconcile

# post-cost per-wallet screen: which wallets clear the cost floor?
python -m analysis.copy_backtest_cli screen
```

**Decision rules**
- **Roster selection** is the dominant lever (wallets are +2%…+87% post-cost
  CLEAR vs -1.4%…-28% NEGATIVE). Promotion/demotion must be cost-aware:
  promote `CLEAR` (net_pct >= 1.5, n >= 20, not one lucky trade); demote
  ACTIVE wallets with post-cost net <= 0.
- **Exit-parameter tuning** (stop/recovery/trailing/time): only grid-search
  against recorded marks once they carry a meaningful sample, and only if
  `reconcile` shows an exploitable mark-vs-fill gap. Today it does not
  (gap ~= cost floor), so exit params are set-and-keep.
- **Sample floor:** never tune on fewer than ~20 shadow positions per unit;
  realized numbers need a meaningful n (today: 186 closes, 4 recent sells —
  insufficient for deferral decisions).

---

## Exit-parameter grid-search

```bash
# default shadow source (reconstructed Birdeye paths, anchored entry)
python -m analysis.exit_grid_search --binary /app/replay_exit --population shadow --limit 5000

# real source: the operator's recorded monitor marks (position_price_marks)
python -m analysis.exit_grid_search --binary /app/replay_exit --source real --limit 5000
```

`replay_exit` is the live exit-rule binary (copied to `/app/replay_exit` from
the operator image). The grid sweeps the params `ProfitManagementConfig`
exposes as overrides (stop distance, recovery gate, trailing, losing/time
exits) and ranks configs by cost-adjusted sum PnL. **Known result:** on
current shadow paths the configs are statistically identical (~92-95% exit via
time_exit flat; cost drag dominates) — exit params are inert on this data, so
the harness is a sensitivity check, not a tuning license.

---

## Paper-optimal roster maintenance

```bash
# show the rebalance the scheduled cycle would apply (safe):
python -m core.shadow_promoter --dry-run
# one-shot apply of the paper-profit rebalance (promote CLEAR, demote burners):
python -m core.shadow_promoter --optimize-paper --apply
```

The **scheduled** promotion cycle (`scout/main.py` -> `shadow_promoter.run_cycle`)
already runs paper-optimal: every cycle promotes post-cost-CLEAR candidates to
ACTIVE and demotes ACTIVE wallets whose post-cost net <= 0 (capped 25/50),
so the copy set continuously tracks the CLEAR optimum even when other roster
maintenance (e.g. inactivity auto-demote) reverts individual wallets.

---

## Forward data collection (the remaining blocker)

`position_price_marks` (migration 0021) records the monitor's USD price mark
for every open position each tick (~5s). It is the missing price history that
makes the realized gap and deferral params tunable **on recorded data** once
realized throughput accrues a meaningful sample. Until positions open and
close with marks, `mark`/`screen`/`reconcile` realized sections will read
empty or sparse — that is the expected state, not a fault.

Watch-points after throughput increases:
- `mark` shows positions with recorded marks and their dip-recovery geometry.
- `reconcile` keeps the price-basis gap near zero (basis faithful) — if it
  diverges, execution/fill quality is the problem, not the shadow model.
- `screen` realized book develops CLEAR wallets (positive net closes).