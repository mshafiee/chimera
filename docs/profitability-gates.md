# Profitability Go/No-Go Gates (Phase C4)

Pre-registered criteria for evaluating whether **paper trading** is profitable
enough to consider **live trading**. These gates are defined *before* any
evaluation so the go/no-go decision cannot be retro-fitted to the data.

The live verdict is served by `GET /api/v1/profitability/verdict` and computed
from the immutable `decision_records` table (Phase C1) joined with closed
`trades`.

## Gates

| Gate | Criterion | Threshold |
|------|-----------|-----------|
| **Sample size** | Complete closed outcomes with all lifecycle events | ≥ 60 |
| **Net return** | Lower 95% confidence bound of net return per deployed SOL | > 0 |
| **Cohort positivity** | Positive net return in every cohort with ≥ 10 outcomes (wallet, strategy, liquidity band, latency band) | All cohorts positive |
| **Paper/live bias** | Declared bias within pre-set bound | ≤ 5% |
| **Max single loss** | Worst single-position loss | ≤ 10% of deployed capital |
| **Max drawdown** | Peak-to-trough drawdown of cumulative PnL | ≤ 20% of deployed capital |
| **Integrity** | Missing-outcome or invalid-PnL trades | 0 |
| **Completeness** | `decision_records` persistence rate | ≥ 99% |

### Gate definitions

- **Sample size:** count of closed outcomes (`trades.status = 'CLOSED'`,
  `pnl_data_valid = TRUE`, side = `'SELL'`) linked to an admitted
  `decision_records` row via `trade_uuid`.
- **Net return:** mean of `net_pnl_sol / deployed_sol` per closed outcome,
  reported as a 95% confidence interval (normal approximation). The lower
  bound must exceed zero.
- **Cohort positivity:** outcomes are grouped by (wallet, strategy, liquidity
  band, latency band). Every cohort with ≥ 10 outcomes must show positive mean
  net return.
- **Paper/live bias:** declared bias from the C3 shadow-fill model
  (`modeled_slippage_pct` stored on `decision_records.price_impact_pct`).
  The mean modeled bias must be ≤ 5%.
- **Max single loss:** the worst single-position loss as a fraction of
  `total_capital_sol`. Must be ≤ 10%.
- **Max drawdown:** peak-to-trough of the cumulative PnL series over closed
  outcomes, as a fraction of `total_capital_sol`. Must be ≤ 20%.
- **Integrity:** count of admitted decisions that have no closed outcome
  (missing) plus trades with `pnl_data_valid = FALSE`. Must be 0.
- **Completeness:** `decision_records.persisted / attempted` from the
  `DecisionRecorder` counters, exposed via the verdict response. Must be
  ≥ 99%.

## Verdict states

- **GO** — every gate passes.
- **INCONCLUSIVE** — any confidence-interval or sample gate fails *without* an
  integrity failure. There is never a silent KILL: insufficient evidence yields
  INCONCLUSIVE, not a negative verdict.
- **STOP** — an integrity or accounting failure (missing outcomes, invalid PnL,
  completeness < 99%). Trading must halt and be investigated.

### Verdict precedence

1. If **integrity** or **completeness** fails → **STOP**.
2. Else if **sample size** < threshold → **INCONCLUSIVE**.
3. Else if any gate fails (net return CI crosses zero, cohort negative, bias
   exceeded, single loss/drawdown exceeded) → **INCONCLUSIVE**.
4. Else → **GO**.

## Scope

- All evaluation is **paper-only** until a GO verdict is recorded.
- The verdict reflects the current `run_id` by default; the endpoint accepts a
  `?run_id=` query to evaluate a prior run.
- The deployed-capital denominator is `SelectionConfig.total_capital_sol` in
  force for the run (recorded as `config_hash`).

## Live enforcement (Phase C)

The verdict is not advisory. When `CHIMERA_PROFITABILITY__REQUIRE_VERDICT_FOR_LIVE=true`
(default) and `CHIMERA_TRADE_MODE=live`, the operator's `VerdictGate`
(`operator/src/engine/verdict_gate.rs`) refuses new **BUY** admissions until the
current run's verdict is **GO**. Behaviour:

- **Fail-closed.** Unknown / INCONCLUSIVE / any non-GO verdict, or a verdict that
  cannot be computed (no DB), blocks new BUYs. Live trading never starts on a
  strategy whose edge is unproven or has decayed.
- **Paper / Devnet are unaffected.** Shadow and paper trading continue regardless,
  so the system keeps generating the evidence needed to reach GO.
- **Exits always proceed.** Only BUY admissions are gated; protective sells are
  never blocked (consistent with the selection engine's rule that exits are never
  gated).
- The gate caches the verdict for 60s to keep the BUY hot path off the database.

## Evidence loop runbook

This is how you tune the admission gates **to the data** rather than to a fixed
percentage target. The shadow trader records a counterfactual mirror_main PnL for
every signal the main system rejected, so "what if we loosened gate X?" is answerable
from real data — no backtest fabrication.

1. **Run the loop** on the production server:

   ```bash
   bash scripts/profitability_loop.sh
   ```

   This prints (A) the rejection funnel — per-gate signal volume, counterfactual win
   rate and net PnL — and (B) the Pareto frontier — the marginal effect of admitting
   each gate's signals on top of the current baseline.

2. **Read the marginal table.** The gate with the highest `netSol` whose `dWin%` is
   not deeply negative is the candidate to loosen: it is currently rejecting signals
   that would have been net-profitable (`lost_profit` > `correct_rej`).

3. **Confirm it is on the Pareto frontier.** Only act on non-dominated moves — a gate
   that raises monthly return but crashes win rate below the verdict's cohort gate is
   dominated and will be blocked by Phase C anyway.

4. **Make the change.** Adjust the matching `CHIMERA_SELECTION__*` env in
   `docker-compose.yml` (e.g. lower `MIRROR_GATE_MIN_SAMPLES`, or raise a
   `*_MAX_DELTA_PCT`), then redeploy per `AGENTS.md`.

5. **Validate, don't target.** Re-run the loop after a full shadow window. The only
   success criterion is a verdict whose 95% net-return CI excludes zero; never tune
   toward an arbitrary win-rate / monthly-return number, which overfits in-sample.
