# Gate-1 Retest: Feasibility Confirmed, Forward Window Instrumented (2026-09-06)

**Question:** can the cluster hypothesis be honestly re-tested now that the
Gate-0 fix (`smart_money_signals`, pre-admission) makes multi-wallet
behaviour observable?

**Answer: YES — and the instrument is now running in production.** But the
sample is still too small for a verdict; the 14-day forward window
(2026-09-06 → 2026-09-20) accumulates it automatically.

## What 5 hours of captured flow showed

| Metric | Value |
|---|---|
| Signals captured | 2,298 (BUY+SELL) across 288 tokens |
| Tracked wallets active | 15 |
| Dispersion-valid cluster triggers (≥3 wallets, 12h, span≥180s, gap≥5s) | **2** |
| Near-miss 2-wallet clusters | 7+ tokens |
| Projected trigger rate | ~10/day → ~140 by 2026-09-20 |

**The observability failure that invalidated the original Gate 1 is fixed:**
the old pipeline structurally never saw ≥3-wallet clusters (5-wallet roster,
admission-gated consensus). The pre-admission recorder sees them within
hours.

## Feasibility probe (1 trigger with live price data)

Simulation of the held plan's entry rule (next hourly close after the 3rd
wallet's buy, GeckoTerminal OHLCV), 2% round-trip cost:
- `6GmAFSYs…` (3 wallets): entry 0.1380 SOL-equiv, +2h: −4.7% gross → −6.7% net
- `XsoCS1Tf…` (3 wallets): no live pool → price unpriced (real failure mode
  the engine would need to handle: dead-pool tokens can't be exited either)

**Context only. n=1 is not evidence in either direction.**

## Forward instrumentation (deployed 2026-09-06)

- Migration `0025_cluster_triggers` (applied on prod directly; sqlx picks it
  up on next operator deploy)
- `scout/scripts/cluster_trigger_capture.py`: idempotent trigger capture
  (UNIQUE token+trigger_at) + GeckoTerminal entry-price backfill
- 15-min cron on chimera-01 (log: `/var/log/cluster_trigger_capture.log`)
- End-to-end verified: 2 triggers recorded, 1 priced

## Verdict protocol (frozen)

On 2026-09-20: join `cluster_triggers` to forward hourly prices at
+2h/+6h/+24h horizons, cost-adjusted (2%), bootstrap 95% CI, same GO bar
semantics as the mirror validation (n≥300 if the trigger rate holds; if
n<100 by window end, extend once by 14 days — logged before looking).
**If cluster EV is again negative with adequate n, the cluster hypothesis is
permanently retired.**

Note: this runs in parallel with the Pivot-A mirror validation window —
independent instruments, no interference.

## Verdict status (2026-09-20): INCONCLUSIVE — roster-blocked

Window closed with **n = 36 triggers** (2026-09-06 → 2026-09-10), far below the
n ≥ 300 bar and below the n < 100 extension threshold. The projection of
~10/day (~140 by 09-20) did not materialise: triggers fell to **zero after
2026-09-10** and none have been recorded since.

The capture cron is alive (`*/15`, log fresh at 06:30 UTC on 09-20) but reports
`inserted_now=0` every run. Root cause is upstream, not the capture job: cluster
detection needs ≥3 tracked wallets in the same token within 12h, and the tracked
roster has collapsed to **5 ACTIVE wallets**. The pipeline is observing correctly
— there is simply no multi-wallet clustering to observe.

Per the frozen protocol the n < 100 case allows **one** 14-day extension, logged
before looking. Extension is **deferred as futile**: without roster recovery,
the trigger rate stays zero and the window cannot accumulate evidence. The
cluster hypothesis therefore has **no verdict** and cannot get one until the
roster is repopulated. This is a data-availability outcome, not a negative
finding about the hypothesis.
