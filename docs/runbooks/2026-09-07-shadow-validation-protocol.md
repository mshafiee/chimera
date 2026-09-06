# Shadow-Validation Protocol (FROZEN 2026-09-07)

- **Strategy under test:** `mirror_main` exits on the Dune cohort
  (`shadow_id LIKE 'dune_%'` rows + wallets present in the refreshed
  bootstrap set).
- **Instrument:** `scout/scripts/cluster_revalidation.py --mirror-validation <days>`
- **GO bar (frozen BEFORE collection):** n ≥ 300, avg pnl > 0, bootstrap 95%
  CI lower bound > 0. Seed `20260907`. Minimum collection: **14 days** from
  the fresh bootstrap `--apply` date.
- **NO threshold changes, window changes, or cohort changes after the first
  look.** Every interim peek must be logged in this file with date + n.
- **On GO:** hand to Gate 2 (engine hardening from the held cluster plan)
  for live-sizing enablement.
- **On PIVOT after the full window:** the dune alpha is regime-dependent;
  revisit with a refreshed source query, NOT with relaxed thresholds.

## Prerequisite

~~The bootstrap `--apply` must have succeeded~~ **AMENDED 2026-09-06 (before
window open — legitimate pre-registration amendment, zero verdict data
exists):** the Dune API is billing-blocked (HTTP 402, datapoint limit; see
`2026-09-07-dune-bootstrap.md`). The window therefore opens on the **existing
cohort** (60 wallets from the 2026-08-07 set, 35 ACTIVE/PROVING and already
webhook-tracked). Rationale: (a) `mirror_main` exits are produced by the live
webhook path and are fresh data regardless of bootstrap age; (b) the
historical `dune_%` rows only feed t-stat gates, not the validation sample;
(c) the amendment is logged before any window data exists.

- **Window:** forward exits only — `2026-09-06` → `2026-09-20`, enforced by
  running `--mirror-validation 14` no earlier than 2026-09-20 (the trailing
  `--days 14` parameter then covers exactly the pre-registered window).
- **Cohort refresh (fresh bootstrap)** remains desirable for later
  hypothesis rounds but is NOT a gate for this window.

## Interim peek log

| Date | Days | n | mean | CI-lo | Meets bar | Notes |
|---|---|---|---|---|---|---|
| 2026-09-06 | 30 | 1297 | +99.5 | (pre-instrument) | — | context only: stale cohort, from pivot-analysis forensics |
| 2026-09-06 | — | — | — | — | — | protocol amendment: window opens on existing cohort (Dune 402 blocker); forward window 2026-09-06 → 2026-09-20 |

## Verdict

PENDING — collection window opened 2026-09-06 (amended). Verdict run no
earlier than 2026-09-20.

| 2026-09-06 | — | — | — | — | — | baseline captured (context: n=1265, mean +86.5%, 30d trailing); signal flow verified: 786/2245 signals from cohort, 0 failures — see docs/superpowers/analysis/2026-09-07-pivot-a-baseline.md |
