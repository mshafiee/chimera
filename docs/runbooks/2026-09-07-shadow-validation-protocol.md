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

The bootstrap `--apply` must have succeeded (fresh `dune_%` rows; see
`2026-09-07-dune-bootstrap.md`). Until then the instrument runs against the
stale 2026-08-07 cohort — any number it prints is context, not verdict.

## Interim peek log

| Date | Days | n | mean | CI-lo | Meets bar | Notes |
|---|---|---|---|---|---|---|
| 2026-09-06 | 30 | 1297 | +99.5 | (pre-instrument) | — | context only: stale cohort, from pivot-analysis forensics |

## Verdict

PENDING — collection window opens on first successful `bootstrap_dune --apply`.
