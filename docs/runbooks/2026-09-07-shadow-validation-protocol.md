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
| 2026-09-19 | 13 | 1971 | +9.03 | +1.47 | yes (peek) | **MECHANICAL GO ONLY — not economically real.** Instrument repaired (4 bugs, below); cohort restored to frozen def. 79% of rows are `no_price` zeros that pad n; the mean is driven by 192 `profit_target_5` winners (avg +135%, max +7960%); trimmed to ≤100% the mean is **−1.23%**; median 0.00. `mirror_main` is unreachable live and shadow marks are fill-optimistic. |

### Instrument repair (2026-09-19, pre-verdict)

The pre-registered instrument could not run as deployed. Four defects fixed
(all before any verdict data existed; the n=0 peek was a filter artifact):

1. `import scout.analysis.db` — the scout image flattens `scout/` to `/app`
   (no `scout` package) → `ModuleNotFoundError`. Fixed with a layout fallback.
2. `if __name__ == "__main__"` sat **above** the Pivot-A function defs →
   `NameError: run_mirror_validation`; the script could never execute.
3. `LIKE 'dune\_%'` — psycopg parses `%` as a placeholder →
   `ProgrammingError`; escaped to `%%`.
4. **Cohort definition contradiction** — `summarize_mirror` re-filtered to
   `shadow_id LIKE 'dune_%'` only, discarding the frozen cohort's
   bootstrap-set-wallet half. The stale `dune_` rows have aged out of the
   window while the cohort wallets keep producing live-path (UUID-keyed)
   exits, so the prefix-only filter collapsed the sample to **n=0** (an
   automatic PIVOT on a bug). Restored to the frozen definition
   (`dune_%` **or** bootstrap-set wallet), with the predicate applied
   defense-in-depth in Python. Fix: commit `4f2f5c0`.

### Robustness gap in the frozen GO bar (flag, do not amend post-hoc)

The bar (n≥300 ∧ mean>0 ∧ CI-lo>0) is passed by a distribution the bar was
never designed to guard: 1,557 of 1,971 exits (79%) are `no_price` zeros that
inflate `n` while contributing nothing; 192 `profit_target_5` moonshots
(one at +7,960%) carry the entire mean; 149 `stop_loss` (−47.9% avg) and 73
`recovery_gate` (−13.5% avg) are the real losses. Excluding >100% exits the
mean is negative. **A GO on this shape must not proceed to live-sizing
without a moonshot-robustness check** — but adding one now would be a
post-hoc threshold change and is therefore logged here as a governance
flag, not applied.


## Verdict

PENDING — collection window opened 2026-09-06 (amended). Verdict run no
earlier than 2026-09-20.

| 2026-09-06 | — | — | — | — | — | baseline captured (context: n=1265, mean +86.5%, 30d trailing); signal flow verified: 786/2245 signals from cohort, 0 failures — see docs/superpowers/analysis/2026-09-07-pivot-a-baseline.md |
