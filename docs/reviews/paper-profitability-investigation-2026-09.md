# Paper-Trading Profitability Investigation — 2026-09-05

**Scope:** Why paper trading is not profitable. All live numbers collected 2026-09-05 15:50–16:40 UTC from `chimera-01.moez.tech` (read-only SQL + operator logs + verdict API + `scripts/profitability_loop.sh`). Plan: `.kilo/plans/1788623416466-paper-profitability-investigation.md`.

## Headline

The system loses money on every window and on every dimension measured, for **compounding reasons**: the admitted cohort itself is net-negative (selection), realized fills are systematically worse than the marks the exits react to (execution), costs are a large fraction of the thin average edge (cost drag), and the winning cohort that *does* exist (post-Sep-2 admission) is repeatedly halted by the circuit breaker. The Sep 2 gate relaxation (`597d6c5`) restored *flow* but not *profitability*.

## 1. The verdict (live, 2026-09-05T16:28Z)

`GET /api/v1/profitability/verdict` → **STOP**

| Gate | Value | Threshold | Status |
|---|---|---|---|
| sample_size | 92 | ≥60 | PASS |
| **net_return** | mean **−2.21%** per deployed SOL, 95% CI −3.52%…−0.90% | lower > 0 | **FAIL** |
| **cohort_positivity** | 0/2 cohorts positive | all | **FAIL** |
| paper_live_bias | 1.36% (winsorized fix working) | ≤5% | PASS |
| max_single_loss | 2.1% | ≤10% | PASS |
| max_drawdown | 12.2% | ≤20% | PASS |
| **integrity** | 62 missing outcomes | 0 | **FAIL** |
| completeness | 100% | ≥99% | PASS |

Bias and completeness gates are now healthy — the Aug-19-era data-quality corruption is fixed. The remaining FAILs are *real economics* (net_return, cohort) plus a small integrity residue (62 admitted decisions without a valid closed outcome; 23 of 170 admitted decisions in 30d have no trade row at all).

## 2. PnL by window and strategy (closed BUYs, `pnl_data_valid`)

| Window | Strategy | n | Win% | Gross SOL | Cost SOL | Net SOL |
|---|---|---|---|---|---|---|
| 24h | SHIELD | 2 | 0.0 | −0.1227 | 0.0194 | **−0.1227** |
| 7d | SHIELD | 10 | 30.0 | −0.1803 | 0.0556 | −0.1177 |
| 7d | SPEAR | 4 | 0.0 | −0.0366 | 0.0235 | −0.0366 |
| 30d | SHIELD | 65 | 26.2 | −1.0855 | 0.3866 | −0.6649 |
| 30d | SPEAR | 15 | 33.3 | −0.2268 | 0.0527 | −0.2030 |
| older | SHIELD | 85 | 17.6 | −0.3591 | 0.3012 | −0.2189 |
| older | SPEAR | 45 | 13.3 | −0.3307 | 0.1699 | −0.3117 |

Position-level (30d, n=96): win 26.0%, avg net **−0.0121 SOL/trade**, total −1.16 SOL, average hold **0.5h**. **Size skew confirmed and worse than the Aug-22 plan assumed:** ≥1.0 SOL entries lose −0.0393/trade (n=35) vs −0.0044 for <0.5 SOL (n=55) — bigger copies lose ~9× more per trade.

7-day rolling: 16 closed, 18.8% win, −0.0173/trade. Both strategies negative in every window; SHIELD carries the book.

## 3. Rejection funnel (7d `decision_records`, live)

Top rejection codes (7d): `WALLET_NOT_ACTIVE` 10,632 · `NO_ACTIVE_POSITION` 9,422 · `LIQUIDITY_BELOW_MINIMUM` 3,176 · `TOKEN_TOO_NEW` 2,423 · `SHADOW_MIRROR_INSUFFICIENT` 1,396 · `PUMP_CHASE` 1,373 · `POSITION_SIZE_ZERO` 856 · `SIGNAL_QUALITY_TOO_LOW` 555 · `TOKEN_UNSAFE` 456 · `ENTRY_DRIFT_EXCEEDED` 335.

Admissions recovered from the Sep-2 fix but remain a trickle: 0→6→3→6→8→17→7 admitted/day (Aug 29–Sep 4), 93 linked of 47 admitted in the last 4 days — vs 25,000+ rejections/week.

### Counterfactual shadow PnL (mirror_main, winsorized)

| Gate | n | Win% | Winsorized avg | Winsorized net SOL | Verdict |
|---|---|---|---|---|---|
| **ADMITTED** | 212 | 40.1 | **−0.31%** | **−0.60** | engine's own cohort still net-negative |
| WQS_TOO_LOW | 498 | 55.6 | +9.62% | +47.9 | over-strict — top loosen candidate |
| TOKEN_UNSAFE | 2,045 | 53.2 | +0.68% | +14.5 | phantom-PnL caveat still un-audited |
| SHADOW_MIRROR_INSUFFICIENT | 504 | 49.4 | +0.88% | +4.4 | mild over-strictness (mirror now OFF) |
| SINGLE_WALLET_UNPROVEN | 486 | 42.2 | +0.84% | +4.1 | mild |
| LIQUIDITY_BELOW_MINIMUM | 623 | 32.4 | +0.93% | +1.8 | mild |
| SIGNAL_QUALITY_TOO_LOW | 599 | 41.2 | −0.15% | −0.8 | protective — keep |
| TOKEN_TOO_NEW | 1,859 | 43.2 | **+0.97%** | **−41.0** | raw +1,218 SOL is 59 moonshots; winsorized NEGATIVE — do NOT loosen (plan was right) |
| WALLET_NOT_ACTIVE | 3,286 | 35.3 | −2.92% | −131.8 | protective — the 10.6k/day rejections are noise filtering |

**Key finding:** the Pareto frontier (scout `analysis.cli frontier`, n=244 baseline: win 42.2%, monthly −4.91%) says the *only* strongly non-dominated loosen moves are `WQS_TOO_LOW` (+9.0% dWin, +47.9 SOL) and `TOKEN_UNSAFE` (+9.9% dWin, +18.6 SOL, pending phantom-PnL audit). `TOKEN_TOO_NEW` and `WALLET_NOT_ACTIVE` raw numbers are moonshot artifacts — winsorized, they are negative.

**The deepest problem is unchanged from the Aug-20 golden baseline: what the engine actually admits is its own worst cohort (ADMITTED: 40.1% win, −0.31%/trade shadow; realized 26% win, −0.012 SOL/trade).** Meanwhile the shadow book overall (`shadow_comparison`, rejected only, 7d): 4,740 signals, 39.0% win, avg +21.05% — the *rejected* pool outperforms the *admitted* pool on expectancy. Selection is inverted at the margin.

## 4. Shadow vs realized (the realize-vs-price gap is still killing the book)

- Shadow book: 25,604 positions; 7d mirror_main exits n=4,771, win 38.8%, avg **+20.9%**.
- Realized closed BUYs (30d): win 26.0%, avg net **−1.21%**.
- Twin pairing now works: `shadow_comparison` has 30 admitted twins (win 50%, +1.285%) vs 4,740 rejected twins — the Phase-2 dedup fix landed and the ADMITTED row is populated. The admitted twin set is still the weaker book (shadow-overstated).
- Position peak-vs-exit: best unrealized +23.2%, worst −5.9% — winners were visible but the engine banked losers; with 0.5h average hold, exits are cutting before the mirror_main edge (which banks within minutes) can differentiate, and the shadow's +20.9% avg rests on illiquid-mark fantasy exits (AbNNre class).

## 5. Execution & costs

- Buy-side cost mix (7d): Jito 0.032% + DEX fees 0.262% + slippage 0.004% = **0.298% per entry**. Round-trip with exit ≈ 0.6–0.9% — vs an admitted-cohort expectancy of −0.31% shadow / −1.21% realized. **Costs are not the dominant loss driver anymore** (gross is negative too: −1.09 SOL/30d SHIELD gross), but at micro sizes (avg position 0.84 SOL, min 0.085) the cost floor ≈ −1.5%/position makes thin edges unbankable.
- DEAD_LETTERs (7d, n=22): 7 honeypot/inconclusive (protective, working), 5 cost-efficiency rejections (Sep-2 fix reduced but did not eliminate), 1 off-hours size floor. NULL-reason rate 7.4% (target ~0, improved from 225).
- Circuit breaker: **tripped 5× in 24h on "5 consecutive losses"** — with 18.8% win rate, 5-loss streaks are near-certain; each trip freezes admissions for cooldown and "27 Helius webhook signals blocked by circuit breaker" were dropped in the last window. Paper book spends much of the day in COOLDOWN (health snapshot: `circuit_breaker.state=COOLDOWN, trading_allowed=false`).
- Helius load is sane post-fix: 123 tombstonings + 307 lite-api fallbacks/24h (was ~27.8k WARN/12h).

## 6. Roster: the signal sources are the problem

29 ACTIVE wallets, but **27 have never traded** (`last_trade_at` NULL — stale dune-bootstrap imports at default WQS 80.0), and the two that do trade are stale: last trades Aug 11 and Jul 26. Actual 7d signal flow comes from wallets that aren't in the ACTIVE set's trading record: 13,998 signals from `132Tkgf…` (WQS 10.0, currently ACTIVE-but-never-traded row), 8,599 from `3nMNd89…` (SCALPER, WQS 64), plus `bpfe9t…` (2,621) and `2qG8…` (1,024, SWING, last trade Aug 11).

Roster funnel: 29 ACTIVE / 34 PROVING (avg WQS 67–89) / 10,919 CANDIDATE (avg WQS 15.0) / 2,027 REJECTED. The candidate pool is huge but near-empty of quality; the ACTIVE set is largely decorative. PROVING (34 wallets) is the real pipeline but stalls: 3-day-old run still shows `WALLET_NOT_ACTIVE` as the #1 rejection — promotion is slower than signal decay.

**WQS-vs-reality divergence:** the top signal producers have WQS 10–64 while 80.0-score wallets sit idle. The scoring model and the actual flow have disconnected.

## 7. Log/infra classes (24h)

- 37 ERRORs — all the same circuit-breaker trip line (5× real trips).
- `Adaptive stop-loss widening overridden by max_stop_loss_distance` — 2,459×/48h: the adaptive stop wants to widen but the config cap forces tight stops; with 0.5h holds and −9.75% avg shadow loss per loser, this is the exit-regime rigidity signature (plan `1785560750000-recovery-gate-exit-strategy` territory).
- Pre-graduation curve fetch failing fail-open 1,393×/48h (pump.fun graduation checks silently skipped — potential unhealthy-token admissions).
- Webhook ingestion healthy: 529 events, 56 parsed swaps, 307 rate-limit fallbacks.

## 8. Confirmed / refuted root causes (vs the local evidence tree)

| # | Hypothesis (from local evidence) | Live verdict |
|---|---|---|
| 1 | Over-rejection from mis-calibrated gates | **Partially fixed, still binding.** Sep-2 relaxation restored flow (0→7–17/day); `SHADOW_MIRROR_INSUFFICIENT` rejections stopped entirely post-deploy (74 on Sep 2, 0 since). But admission is still ~0.4% of signals, and the biggest residuals are `WALLET_NOT_ACTIVE` (roster, not gates) and `LIQUIDITY_BELOW_MINIMUM` (~600/day at the new $10k floor). |
| 2 | Admitted cohort is net-negative (inverted selection) | **CONFIRMED, still true.** ADMITTED shadow cohort: 40.1% win, −0.31%/trade; realized 26.0% win, −0.012 SOL/trade. Rejected pool shows +21% avg shadow. The engine admits worse-than-noise at the margin. |
| 3 | Execution gap (fills vs marks) | **Still open.** Shadow +20.9% avg vs realized −1.21%; exit-execution-truth fixes shipped but the gap persists in the aggregate; 2,459 adaptive-stop overrides/48h show the exit regime is capped tight while trades die at −9.75% avg shadow loss. |
| 4 | Shadow measurement corruption | **Fixed.** Bias gate 1.36% (was 22.99% corrupted); admitted twins now recorded (30 rows); completeness 100%. Shadow remains fill-optimistic for illiquid exits by design. |
| 5 | Cost drag | **Demoted to secondary.** Gross is negative before costs (−1.09 SOL/30d gross vs −0.66 net SHIELD). Costs 0.30%/entry are no longer the leading loss term at current sizes, but the −1.5% floor still dominates any thin positive edge. |
| 6 | Churn | **Largely resolved.** Only 3 tokens re-entered in 7d (SL cooldown working; was 72% re-entries). |
| 7 | Roster quality | **CONFIRMED, now the #1 structural issue.** 27/29 ACTIVE wallets never traded; signal flow dominated by 4 wallets, top one WQS 10.0; PROVING pipeline stalls behind `WALLET_NOT_ACTIVE` rejections (10.6k/week). |
| 8 | Infra noise | **Fixed.** Jupiter flood gone (123 tombstones/24h); Helius 429s at fallback level only. New: circuit-breaker thrash (5 trips/24h) is the new flow-killer. |

## 9. Current dominant loss drivers, ranked (live-measured)

1. **Negative-EV admission: the engine's own cohort.** Admitted trades lose −1.2% net/trade realized while the surrounding rejected pool shows positive shadow expectancy. This is a *wallet/entry selection* failure, not a gate-strictness failure — tightening further makes it worse, loosening admits more noise.
2. **Roster → signal-source collapse.** 93% of ACTIVE wallets never trade; flow rides on 4 wallets (one at WQS 10); PROVING wallets can't reach ACTIVE before their edge decays (`WALLET_NOT_ACTIVE` 10.6k/week).
3. **Exit regime rigidity vs the 0.5h book.** Hold time 0.5h; avg shadow loser −9.75%; 2,459 adaptive-stop overrides/48h; shadow shows +20.9% avg for mirror_main on the same signals — the live exit rail is not capturing what the shadow rail shows.
4. **Circuit-breaker thrash.** 5-loss streak threshold trips ~5×/day at an 18.8% win rate; each trip drops live signals (27 blocked in the last window) — the system is offline a material fraction of the day.
5. **Cost floor vs micro sizes.** ~1.5%/position round-trip cost floor at avg 0.84 SOL; a 26%-win book needs >1.4% avg edge to clear it, and the book averages −1.2%.

## Appendix A — Remediation sequence (references existing plans only; nothing here invents new strategy parameters)

1. **Stabilize flow first (circuit-breaker thrash).** At 18.8% win rate, a 5-consecutive-loss trip is a certainty, not protection. Either raise the consecutive-loss threshold for paper mode or count calendar-day net loss instead of streaks. (Sizing/exit-adjacent change → `🛡️ safety:` + tests per repo policy.) Until then, every other fix's measurement window is fragmented.
2. **Fix the roster (`.kilo/plans/1785443061762-fast-track-promotion.md` + zero-yield rotation from `e71bc33`).** Demote/park the 27 never-traded ACTIVE wallets; fast-track the 34 PROVING wallets that are generating signals; reconcile the WQS model against actual flow (top producer at WQS 10.0 means the score or the promotion gate is broken). This converts 10.6k/week `WALLET_NOT_ACTIVE` rejections into real decisions.
3. **Admission EV at the margin (`.kilo/plans/1787167274286` Phase 2).** With the mirror gate already off, the remaining lever is letting the price-hold bypass also cover `SINGLE_WALLET_UNPROVEN` + `SHADOW_MIRROR_INSUFFICIENT` (n=504+486, ~50% win, +0.85% avg shadow) — the only remaining over-strict gates. Do NOT touch `TOKEN_TOO_NEW` (winsorized −41 SOL) or `WALLET_NOT_ACTIVE`.
4. **Exit regime (`.kilo/plans/1785560750000-recovery-gate-exit-strategy.md` + `1787530000000-exit-execution-truth-plan.md` follow-through).** The 0.5h hold + capped adaptive stops + −9.75% avg shadow loss says the live exit rail is capturing the left tail while shadow shows the right tail (+20.9%). Re-measure `reconcile` post-Fix A/B; the price-path replay harness exists to grid-search exits against the golden baseline (`scout/analysis/golden_baseline_2026-08-20.md`).
5. **Then, and only then, the one-gate-per-window evidence loop (`docs/profitability-gates.md` runbook)** — `WQS_TOO_LOW` is the measured top loosen candidate (+9.0% dWin, +47.9 SOL winsorized, n=498). `TOKEN_UNSAFE` stays blocked until the phantom-PnL audit samples its exits (unfillable sell legs).

**Success criterion (unchanged):** verdict `net_return` 95% CI excluding zero. Validate, don't target.

## Appendix B — Reproduction

Collection queries (read-only, run on `chimera-01` in `/opt/chimera`):
- `bash scripts/profitability_loop.sh` (funnel A; frontier B needs `docker exec chimera-scout sh -c 'cd /app && DATABASE_URL=postgres://chimera:chimera@postgres:5432/chimera python -m analysis.cli frontier'` — host venv lacks `psycopg`; conversion funnel C)
- `curl -s http://localhost:8080/api/v1/profitability/verdict`
- Window/strategy PnL: `SELECT CASE ... '24h'/'7d'/'30d' ... , strategy, count(*), win%, sum(pnl_sol), sum(total_cost_sol), sum(net_pnl_sol) FROM trades WHERE side='BUY' AND status='CLOSED' AND pnl_data_valid GROUP BY 1,2`
- Rejections: `SELECT rejection_code, count(*) FROM decision_records WHERE NOT admitted AND decided_at > now()-'7 days' GROUP BY 1`
- Twins: `SELECT main_admitted, count(*), win%, avg(pnl_pct) FROM shadow_comparison WHERE exit_strategy='mirror_main' GROUP BY 1`
- Roster: `SELECT status, count(*), avg(wqs_score) FROM wallets GROUP BY 1` + `SELECT count(*) FILTER (WHERE last_trade_at IS NULL) FROM wallets WHERE status='ACTIVE'`
- Churn: `SELECT token_address, count(*) FROM positions WHERE opened_at > now()-'7 days' GROUP BY 1 HAVING count(*)>1`
