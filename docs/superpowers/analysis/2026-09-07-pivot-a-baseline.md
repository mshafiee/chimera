# Pivot-A Validation Baseline (2026-09-06)

**Purpose:** frozen context snapshot before the pre-registered window
(2026-09-06 → 2026-09-20). These numbers are CONTEXT ONLY — they are not the
verdict. The verdict is the `--mirror-validation 14` run on/after
2026-09-20 (forward exits within the window only).

## Instrument sanity check (stale-cohort context, 30d trailing)

```
mirror_main exits, dune cohort, last 30 days:
  n       = 1,265
  mean    = +86.54%
  median  = 0.00   (fat right tail — lotto-shaped, as the pivot analysis noted)
```

Matches the 2026-09-06 pivot-analysis forensics (n=1,297, mean +99.5% at a
slightly earlier cutoff). Instrument and SQL reproduce the audit numbers.

## smart_money_signals flow (Task 5 early check, ~5h after deploy)

| Metric | Value |
|---|---|
| Total signals captured | 2,245 |
| From dune-cohort wallets | **786 (35%)** |
| Recording failures since 0024 fix | 0 |

The 60 dune wallets produce ~35% of all captured flow — the validation
window's data source is live and healthy. 48h follow-up check scheduled per
Task 5 of the plan (`docs/superpowers/plans/2026-09-07-pivot-a-dune-copy-first.md`).

## Window mechanics

- Cohort: existing dune set (60 wallets; 6 ACTIVE / 29 PROVING / 22 CANDIDATE
  / 3 REJECTED at window open). Amendment rationale:
  `docs/runbooks/2026-09-07-shadow-validation-protocol.md`.
- Bootstrap refresh: BLOCKED — Dune API 402 (datapoint limit). Not a gate for
  this window. Re-run `bootstrap_dune --apply --roster` after billing reset
  and log it in the bootstrap runbook.
- Verdict run (no earlier than 2026-09-20): rebuild scout image with the
  committed script, then
  `docker compose exec -T scout python -m scout.scripts.cluster_revalidation --mirror-validation 14`
  (exit 0 = GO, exit 1 = PIVOT). Record output + git sha in the protocol
  verdict section.

## Metadata

- git sha `0904c43`+ (instrument), prod operator deployed from `c2d9a47`
- DB snapshot 2026-09-06 ~21:0x UTC+2, chimera-01.moez.tech
