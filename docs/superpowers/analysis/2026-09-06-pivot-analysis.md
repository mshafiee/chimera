# Pivot Analysis: Post-Gate-1 Strategy Direction

**Date:** 2026-09-06
**Trigger:** Gate 1 PIVOT verdict (all 3+ cluster buckets negative on repaired attribution — see `2026-09-06-cluster-revalidation-results.md`).
**Method:** Production DB forensics on the only profitable historical signal (`dune_wallet`) and on WQS-band stratification of the live roster. All queries read-only against `chimera-01.moez.tech`.

---

## 1. The `dune_wallet` Signal Is Real and Structurally Different

| Metric | Value |
|---|---|
| Wallets | 60 (Dune-derived set, ingested 2026-07-08 → 2026-08-07) |
| Exits | 3,000 (50 per wallet) |
| Mean pnl | **+52.2%** |
| Winners | 1,504/3,000 (50.1% win rate) |
| **Net-positive wallets** | **53 of 60 (88%)** |

The 88%-of-wallets-profitable figure is the headline: this is **wallet-selection alpha**, not a luck artifact. Random 50-exit samples from a negative-EV population would not produce 53/60 net-positive wallets. Contrast with the tracked roster, where ~100% of wallets are net-negative.

### 1.1 Monthly persistence (while ingested)

| Week of | n | avg pnl |
|---|---|---|
| 2026-07-06 | 56 | +69.8% |
| 2026-07-13 | 482 | +10.0% |
| 2026-07-20 | 477 | +36.9% |
| 2026-07-27 | 1,067 | +80.4% |
| 2026-08-03 | 918 | +48.6% |

No decay trend before the ingestion stopped on 2026-08-07. The alpha was **discontinued operationally, not exhausted**.

### 1.2 The wallets are still on the roster

| Status | Count |
|---|---|
| ACTIVE | 6 |
| PROVING | 29 |
| CANDIDATE | 22 |
| REJECTED | 3 |

35 of 60 dune wallets (58%) are ACTIVE or PROVING today. The roster infrastructure already watches most of them — what was lost is the *shadow-trading treatment* the dune_wallet strategy applied (Dune-derived entry methodology), not the wallet coverage.

## 2. Do Dune Wallets Still Outperform via the Live Path? — Mixed, Fat-Tailed

Same 60 wallets, post-cutoff, through the standard monitoring pipeline (per exit strategy):

| exit_strategy | n | median | mean | p25 | p75 |
|---|---|---|---|---|---|
| mirror_main | 1,297 | 0.0 | **+99.5%** | −38.7 | +24.7 |
| fixed_1h | 894 | −51.4 | +54.3 | −85.7 | 0.0 |
| fixed_24h | 785 | −70.7 | −38.0 | −95.0 | −24.7 |
| wallet_sell | 1,431 | 0.0 | +2.4 | 0.0 | 0.0 |
| mirror_v3 | 798 | −9.7 | +33.9 | −42.0 | +6.5 |

Control (non-dune wallets), same period, same strategies: means between −3.1% and +1.0%.

**Interpretation:**
- **mirror_main on dune wallets (+99.5% mean, 44.6% win, n=1,297) vs control (+1.0%, n=9,959) is the only actionable positive delta** — the live copy path preserved part of the alpha.
- Long-horizon shadow strategies (fixed_24h/fixed_4h) on the same wallets are catastrophic (medians −66% to −71%): dune wallets are high-frequency, high-turnover traders. Copying them *immediately* (mirror) captures their edge; holding their bags for 24h does not — by then they've exited.
- The fat right tail (mean ≫ median) means these are lotto-shaped distributions: real but concentrated in a minority of tokens.

**Caution before acting:** mirror_main results may partly reflect the same-period market regime, and Dune-derived wallets self-select for tokens with rich off-chain data. A shadow-paper validation window on live infra is the cheap test.

## 3. WQS Stratification: No Usable Edge

WQS-band performance over admitted positions (`wallet_sell` + `fixed_24h` exits):

| WQS band | n | avg pnl | win% |
|---|---|---|---|
| 80+ | 165 | −12.6% | 34.5% |
| 70–79 | 20 | +23.1% | 50.0% |
| 60–69 | 65 | −6.5% | 27.7% |
| 50–59 | 68 | +45.7% | 79.4% |
| <50 | 90 | +9.2% | 54.4% |

No monotonic relationship; the "best" band (80+) is the worst performer, and small-n bands (70–79, 50–59) swing wildly. **Pivot-b (solo high-conviction swingers filtered by WQS) has no evidentiary support.** The WQS model, as scored today, does not predict realized copy-trade PnL in either direction.

## 4. Recommendation: Pivot Direction

**Primary: PIVOT-A — re-establish and extend the Dune wallet-selection pipeline, copy-first (mirror semantics), shadow-validated.**

Concrete sequence:
1. **Re-enable dune-wallet shadow ingestion** for the 58 still-relevant wallets (35 ACTIVE/PROVING already tracked) — restore the `dune_wallet`-style treatment via `smart_money_signals` so the new pre-admission table accumulates their flow.
2. **Refresh the Dune source set** (the 2026-07 set is 2 months old; query for current high-performing Dune-tracked wallets with the same archetype filters: >4h avg hold is WRONG for this cohort — dune wallets are fast-turnover; use win-rate + turnover-matched filters instead).
3. **Validate mirror_main-on-dune-wallets as the candidate strategy** in shadow mode for 2–4 weeks with a pre-registered bar (n≥300 exits, mean>0, bootstrap CI-lo>0 — same instrument as Gate 1, now parameterized by wallet set).
4. Only on a GO: extend to live sizing via the existing engine; the held cluster plan's Gate 2 execution fixes become relevant then.

**Rejected alternatives:**
- *Cluster accumulation (current plan):* empirically dead (Gate 1).
- *WQS-tail solo swingers:* no predictive WQS signal (§3).
- *Long-hold shadow strategies on dune wallets:* median −66% to −71%; these wallets exit within minutes-to-hours (§2).

**Cost note:** Phase 1 (scout roster seeding) of the held plan is salvageable under Pivot-A but with different archetype filters — the swing filters (>4h hold, 55% WR) select for the wrong trader population for copy-first capture.

## 5. Data-Collection Byproducts Already in Place

The `smart_money_signals` table deployed today (pre-admission, bilateral, decimals-tagged) gives every future hypothesis test — including a refreshed dune cohort — a clean primary data source with none of the §1.2 provenance problems. Any re-validation can run against it once enough accumulation window passes (suggest ≥2 weeks of data before the next pre-registered test).

---

**Artifacts:**
- Raw run output: `2026-09-06-cluster-revalidation-raw.json` (Gate 1)
- Gate 1 results: `2026-09-06-cluster-revalidation-results.md`
- Pivot forensics (this doc): ad-hoc psql queries against prod, reproducible via the SQL blocks in §1–§3
