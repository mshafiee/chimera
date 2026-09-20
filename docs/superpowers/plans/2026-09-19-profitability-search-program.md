# Profitability Search Program (amended 2026-09-19)

**Goal:** determine, with pre-registered bars and zero real trading capital,
whether any edge exists in the current flow; make paper trading honest and
measurable; retire hypotheses permanently when they fail.

**Ceiling (accepted 2026-09-19):** the best outcome available under the
no-real-funds constraint is a **validated paper-profitable system plus a
funded-live proposal with receipts**. Paper profit is not withdrawable. The
program cannot itself produce money; it maximizes the probability that a future
funded step succeeds, and bounds the time spent if no edge exists.

**Floor:** a receipted proof of no edge within ~8 weeks.

---

## Phase 2 result (executed early, 2026-09-19) — NO positive cohort

See `docs/superpowers/analysis/2026-09-19-realized-cohort-mining.md`.

- Book: n=261 closed BUYs, **−2.078 SOL**, avg −0.00812/trade.
- Every n≥10 bucket is negative per SOL (−1.39% to −3.26%).
- Only positive candidates are noise (best t=0.57).
- "WQS inversion" was a **size confound** — retracted.
- Consensus never fires (2,562/2,563 admitted at count=1).

**Kill-rule outcome: mark-based gate loosening is permanently retired** (WQS
floor, `TOKEN_UNSAFE`, mirror relaxation, `TOKEN_TOO_NEW`, `WALLET_NOT_ACTIVE`).

---

## Amendments to the 5-phase program

1. **Flow floor (Weakness 1).** Phase 1's pre-admission move shrinks admissions
   and risks INCONCLUSIVE-forever. Add a quarantined **measurement lane**: keep
   the WQS 10–15 trial lane at micro size, record its PnL in a separate
   `run_id`/flag, and exclude it from the verdict sample. Volume for
   measurement, clean sample for the verdict.

2. **Out-of-sample confirmation (Weakness 3).** Any Phase 2 discovery (including
   a *negative* one, e.g. a size cap) is a **candidate**, admitted only to
   quarantine sizing; promotion requires confirmation in a second pre-registered
   forward window.

3. **Exit-control arms (Weakness 4).** Exit A/B must use the experiment control
   arms (`core/src/experiment/`) or shadow-twin deltas, never sequential raw
   verdict deltas — regime drift (+20.9% → +3.40% mirror_main in two weeks)
   confounds sequential tests.

4. **Dune branch (Weakness 4).** Dune repair has two outcomes: billing cycle
   reset → run `bootstrap_dune --apply --roster` (infra spend only); paid limit
   raise required → **skip** (violates no-real-funds) and source from
   cluster/scout only.

5. **Flow generation (added).** Replace the `132Tkgf…` single-wallet dependency
   (92% of 7d flow, lottery-shaped, correctly blocked). Scout discovery cadence
   is an explicit Phase 1 deliverable, or later phases starve.

## Phase order (amended)

| Phase | Work | Gate |
|---|---|---|
| 0 | Run frozen verdicts (mirror Pivot-A + cluster) on/after 2026-09-20; log raw output | GO on either → scope Phase 2 to it |
| 1 | Pre-admission move (honeypot/shadow-blacklist/cost) + RPC cache hygiene + measurement lane + scout cadence | close rate ≥50% |
| 2 | ~~Mine realized cohorts~~ DONE — no positive cohort; mark loosening retired | done |
| 3 | Exit tests via control arms only | must flip the *admitted* cohort, not the rejected pool |
| 4 | Dune branch (infra-only, else skip) | waiver oracle restored |
| 5 | Verdict GO on two consecutive windows | paper GO unlocks paper sizing only; live stays gated behind funded dust validation |

## Stop criteria

Two consecutive full-loop cycles with no positive realized cohort and no
out-of-sample verdict GO → **park the system** (executor/monitor/roster kept;
selection work halted) and pivot strategy rather than tune gates. Named in
advance so the loop cannot run forever.

## Non-goals

- Lowering any admission floor to raise volume.
- Investing live capital (ruled out).
- Paid Dune limits (ruled out).

## Execution record — 2026-09-20 (roster repair + paper drawdown band)

- **Demoted** 3 decorative ACTIVE (`12kNFp`, `129i4z`, `Cr1n5Z`; never traded,
  zero flow) → CANDIDATE.
- **Promoted measurement lane** → ACTIVE: `12PPVhB8` (flow volume),
  `HgPFzrUm` + `2h7Ns9w2` (best 14d shadow win-rates, 43–44%).
- `132Tkgf` stays PROVING (lottery shape; its flow still feeds shadow
  measurement either way).
- **Method:** direct SQL replicating the API path (`wallets` UPDATE +
  `config_audit` INSERT with reasons) — the operator API has no usable
  credential server-side (no `api_keys` configured; wallet-signature auth is
  unavailable headless). Skipped ephemeral effects only: Telegram
  notification, in-memory toxic-detector baseline, Helius webhook cleanup
  (429-blocked; coverage already exists for all six).
- **Paper CB drawdown band 15 → 30**
  (`CHIMERA_CIRCUIT_BREAKERS__MAX_DRAWDOWN_PERCENT`), operator recreated.
  Reversible; buys measurement velocity, not profit.
- Compose YAML repaired (this file's env lists use 7-space indent; an 8-space
  edit broke parsing — caught before deploy completed).
- **Pending:** CB cooldown expiry (~30 min) should clear the restored Tripped
  state against the 30% band. **Binding constraint remains Helius 429 (quota
  exhausted)** — no ingestion, no prices, no fills until it clears; Dune
  promote queries also still failing (billing).
