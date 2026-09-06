# Cluster re-validation (30d, 12h window)

| strategy | mode | bucket | n | win% | avg pnl | CI95 |
|---|---|---|---|---|---|---|
| wallet_sell | all | 1 | 8542 | 25.9% | -2.08 | [-3.22, -0.88] |
| wallet_sell | all | 2 | 1497 | 33.1% | -6.46 | [-9.32, -3.25] |
| wallet_sell | all | 3+ | 637 | 27.3% | -6.13 | [-9.17, -2.54] |
| wallet_sell | dispersion | 3+ | 605 | 27.8% | -5.37 | [-8.37, -2.13] |
| fixed_24h | all | 1 | 7441 | 31.8% | -7.82 | [-9.92, -5.24] |
| fixed_24h | all | 2 | 1442 | 38.8% | -12.65 | [-15.77, -9.09] |
| fixed_24h | all | 3+ | 602 | 36.4% | -9.22 | [-13.19, -5.07] |
| fixed_24h | dispersion | 3+ | 570 | 37.7% | -7.86 | [-11.94, -3.27] |
| fixed_4h | all | 1 | 8024 | 36.1% | -5.11 | [-6.72, -3.00] |
| fixed_4h | all | 2 | 1514 | 39.0% | -9.09 | [-10.90, -7.20] |
| fixed_4h | all | 3+ | 646 | 41.6% | -1.04 | [-3.76, +1.68] |
| fixed_4h | dispersion | 3+ | 614 | 40.1% | -3.29 | [-5.64, -1.10] |

## Verdict

PIVOT: no 3+ cluster bucket cleared the pre-registered bar. The cluster-accumulation engine stays HELD; strategy pivot review required (e.g. solo high-conviction swingers / dune_wallet revival).

## Verdict (pre-registered rule applied)

- Rule (frozen before run): GO iff any wallet_sell/fixed_24h/fixed_4h "3+"
  dispersion-valid bucket has n ≥ 300 AND avg pnl > 0 AND bootstrap 95% CI
  lower bound > 0. Script: scout/scripts/cluster_revalidation.py
  (--days 30 --bootstrap-n 2000 --seed 20260906).
- Outcome: **PIVOT** (script exit code 1). All three strategies' 3+
  dispersion-valid buckets are negative: wallet_sell −5.37% [−8.37, −2.13],
  fixed_24h −7.86% [−11.94, −3.27], fixed_4h −3.28% [−5.63, −0.92].
  The best cluster bucket (fixed_4h all-mode 3+, −1.03%) still has a CI
  upper bound of +1.68 — indistinguishable from zero at best.
- Notable inversion: 2-wallet pairs are the WORST bucket everywhere
  (−6.46% to −12.65%) — consistent with the plan's dev+burner hypothesis,
  but 3+ clusters do NOT recover (contra §1.2's claimed +3.45%/+4.56%).
- Solo baseline is also negative on this roster — the edge is not in
  cluster size at all; it is in wallet selection (cf. retired dune_wallet:
  +52% avg over 3,000 exits, ended 2026-08-07).
- Decision: **cluster-accumulation engine stays HELD. Strategy pivot review
  required** — candidate directions: (a) revive/extend the dune_wallet
  source set with current on-chain validation, (b) solo high-conviction
  swing wallets with WQS-tail filtering, (c) spend a data-collection period
  on the new smart_money_signals table before re-testing with pre-admission
  signals.
- Run metadata: git sha c2d9a47 (deployed prod), scout container on
  chimera-01.moez.tech, DB snapshot 2026-09-06 ~16:3x UTC, raw JSON in
  2026-09-06-cluster-revalidation-raw.json.
