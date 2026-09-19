# Mirror Robustness Gate — Pre-Registration (FROZEN 2026-09-19)

**Purpose:** close the design gap in the 2026-09-07 GO bar, which a
moonshot-driven, dead-row-padded distribution can satisfy. This gate is an
**additional** bar for the **next** collection window. It is **not** applied
retroactively to the window closing 2026-09-20.

**Motivation (context, not verdict):** the 2026-09-19 interim peek met the
legacy bar (n=1,971, mean +9.03, CI-lo +1.47) while being 79% `no_price` zeros,
carrying its entire mean on 192 `profit_target_5` exits (one at +7,960%), with
a **median of 0.00** and a trimmed (≤100%) mean of **−1.23%**. The legacy bar
counts dead rows toward `n` and does not distinguish tail from body.

**Window:** forward exits only, `2026-09-20` → `2026-10-04`. Verdict run no
earlier than `2026-10-04` with `--robustness-validation 14`.

**Cohort (unchanged from 2026-09-07):** `mirror_main` exits on the Dune cohort
= `shadow_id LIKE 'dune_%'` positions **plus** positions of wallets present in
the dune bootstrap set. No cohort changes.

**Seed:** `20260920` (bootstrap 2,000 resamples). **Pre-registered round-trip
cost:** `2.0%` per exit.

## Gates (ALL must pass)

Let the **priced sample** = cohort exits with `exit_reason <> 'no_price'`.

| # | Gate | Criterion |
|---|------|-----------|
| G1 | Priced sample | `n_priced ≥ 300` (dead/`no_price` rows do **not** count toward n) |
| G2 | Winsorized mean | mean of `clamp(pnl_pct, −100, +100)` **> 0** |
| G3 | Winsorized CI | bootstrap 95% CI **lower bound > 0** on the winsorized series |
| G4 | Median | `median(pnl_pct)` over the priced sample **> 0** |
| G5 | Cost-adjusted | winsorized mean **minus 2.0%** round-trip **> 0** |
| G6 | Live-reachable rail | G1–G4 also pass on the **live exit rail** (`wallet_sell`) over the same cohort/window |

G6 is the economic-realism gate: `mirror_main` is a shadow exit **unreachable in
live trading** (the live rail is `wallet_sell` / profit-management). An edge that
exists only on an unreachable rail is not deployable.

## Verdict rule

- **GO** — G1–G6 all pass. Hand to Gate 2 with a live-sizing proposal.
- **PIVOT** — G1 passes but any of G2–G6 fail. The mirror hypothesis is retired
  as a live-sizing candidate; shadow collection continues for research only.
- **INCONCLUSIVE** — G1 fails (`n_priced < 300`). Extend the window **once** by
  14 days, logged before looking, per the 2026-09-07 precedent.

## Anti-tuning clause

No threshold, window, cohort, seed, or cost changes after the first look at the
window's data. Every interim peek is logged in this file with date + n. A GO on
the legacy bar alone (without G1–G6) is explicitly **not** sufficient.

## Instrument

`scout/scripts/cluster_revalidation.py::--robustness-validation <days>` →
`run_robustness_validation(days)` returning per-rail metrics for `mirror_main`
and `wallet_sell` plus `meets_robustness_bar`. The legacy
`--mirror-validation` path is left byte-identical for the 2026-09-20 verdict.

## Non-goals

- Applying this gate to the 2026-09-20 verdict (post-hoc amendment — forbidden).
- Lowering any gate to manufacture a GO.
- Live capital (still ruled out).

## Pre-window context (2026-09-19 diagnostic — NOT a verdict)

Run against the *closing* window (13d) only to confirm the gate discriminates.
Result: **`meets_robustness_bar = false`**.

| Rail | n_total | n_priced | winsorized mean | CI-lo | median | cost-adj | win% |
|---|---|---|---|---|---|---|---|
| `mirror_main` | 1,971 | **414** | +0.97 | **−3.81** | **−8.17** | **−1.03** | 46.4 |
| `wallet_sell` (live) | 2,021 | 464 | **−4.55** | **−7.03** | **0.00** | **−6.55** | 6.5 |

The legacy bar's GO on the same window was a moonshot artifact: once `no_price`
zeros are excluded (n 1,971 → 414), the median turns negative, the CI crosses
zero, and the live-reachable rail is clearly negative. The gate discriminates
as designed. This diagnostic does not consume the next window's first look.

