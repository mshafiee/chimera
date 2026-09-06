# Pivot-A: Dune Wallet-Selection Copy-First Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ## Execution status (updated 2026-09-06)
>
> | Task | Status | Notes |
> |---|---|---|
> | 1 — Dune config restore | ✅ DONE (redirected) | Config already existed on prod (`bootstrap_query_id: 8256459`, key in `/opt/chimera/.env`). Runbook written: `docs/runbooks/2026-09-07-dune-bootstrap.md`. Dry-run executed → **⛔ HTTP 402 datapoint-limit exhausted** (fail-closed, no writes). Blocked on Dune account action: raise datapoint limit or wait for cycle reset. |
> | 2 — DunePnlMonitor re-enable | ⏸ DEFERRED behind Task 1 | Monitor ships in `api/src/main.rs:1237`; needs `PNL_QUERY_ID` live + operator restart. Revisit after the 402 clears. |
> | 3 — Pre-registered instrument | ✅ DONE | `evaluate_go_bar` / `summarize_mirror` / `run_mirror_validation` + `--mirror-validation` CLI in `scout/scripts/cluster_revalidation.py` (14/14 tests pass, incl. brute-force oracle). Protocol frozen: `docs/runbooks/2026-09-07-shadow-validation-protocol.md`. |
> | 4 — Baseline + calendar | ✅ DONE | `docs/superpowers/analysis/2026-09-07-pivot-a-baseline.md`. Window opened 2026-09-06 on the existing cohort (protocol amended pre-window: bootstrap refresh is NOT a gate — mirror_main exits flow regardless). Verdict run: **2026-09-20**. |
> | 5 — Flow capture verification | ✅ DONE (early) | 2,245 signals captured, 786 (35%) from dune cohort, 0 failures. 48h follow-up check 2026-09-08. |
>
> **Additional (2026-09-06): Gate-1 cluster retest instrumented in parallel.** Cluster triggers (≥3 wallets, 12h, dispersion-valid) are observable in `smart_money_signals` — 2 triggers in 5h (~10/day projected). Migration `0025_cluster_triggers` applied on prod; `scout/scripts/cluster_trigger_capture.py` + 15-min cron live; image rebuilt with script baked in (no `docker cp` drift). Feasibility doc: `docs/superpowers/analysis/2026-09-06-gate1-retest-feasibility.md`. Cluster verdict also 2026-09-20 (+14d extension only if n<100, logged pre-look).
>
> **Next touchpoints:** 2026-09-08 (48h flow check) · 2026-09-20 (both frozen verdicts) · Dune billing reset (operator action, unblocks Task 2 + cohort refresh).

**Goal:** Re-establish the empirically-validated Dune wallet-selection alpha (53/60 wallets net positive, +52.2% mean, no decay at cutoff) as a **copy-first (mirror) strategy**, shadow-validated on live infra with a pre-registered GO bar before any live sizing.

**Architecture:** Everything needed already exists and was decommissioned by config decay, not code rot: `api/src/bin/bootstrap_dune.rs` (one-shot Dune → `shadow_positions`/`shadow_exits` ingestion), `operator/src/engine/dune_monitor.rs` (promote/demote cycle), and `mirror_main` (the live copy path that preserved the alpha: +99.5% mean vs +1.0% control). The plan has three workstreams: (1) reconfigure + run the bootstrap to refresh the Dune wallet set, (2) restore the periodic Dune PnL monitor, (3) run a 2–4 week pre-registered shadow validation of `mirror_main` on the dune cohort using the Gate-1 instrument (`scout/scripts/cluster_revalidation.py`'s bootstrap-CI machinery), extended to bucket by wallet set. The held cluster plan stays HELD throughout; no Gate 2 engine work happens until this plan's GO.

**Tech Stack:** Rust 2021 (existing `bootstrap_dune`/`DunePnlMonitor` — no new binaries), PostgreSQL (`shadow_positions`/`shadow_exits`/`wallets`), Dune Analytics API v1, Python (validation script extension), docker compose on `chimera-01.moez.tech`.

## Global Constraints

- **No live sizing changes.** `position_sizer.rs`, `executor.rs`, `signal_pipeline.rs` untouched (operator/AGENTS.md boundary). Shadow-only until the GO bar clears.
- **Secrets:** DUNE_API_KEY lives only in prod env (`docker/env.mainnet-prod` or vault), never in git, never logged.
- **Dune credits are quota-sensitive:** bootstrap is bounded by `bootstrap_wallets_max` (default 60); `--dry-run` default; fail-closed on API errors (existing binary behavior — keep it).
- **Financial precision:** Decimal everywhere; the validation script's `pnl_pct` float cast remains a statistics-only exception as documented in `cluster_revalidation.py`.
- **Pre-registered GO bar (frozen before the validation window starts, not after):** `mirror_main` exits on dune-cohort wallets, ≥300 exits, mean pnl > 0, bootstrap 95% CI lower bound > 0, using seed `20260907`, ≥14 days of collection. Same instrument semantics as Gate 1.
- Every task ends with a commit; prod ops steps are explicit SSH commands.

---

### Task 1: Restore Dune config on production

**Files:**
- Modify: `docker/env.mainnet-prod` (add DUNE_* keys — values only, no secrets committed)
- Create: `docs/runbooks/2026-09-07-dune-bootstrap.md` (ops runbook with the secret-bearing values)

**Interfaces:**
- Consumes: `DuneConfig` in `core/src/config.rs:2874` — fields `enabled`, `pnl_query_id`, `bootstrap_query_id`, `bootstrap_wallets_max`, `bootstrap_roster_enabled`.
- Produces: env keys `CHIMERA_DUNE__ENABLED`, `CHIMERA_DUNE__PNL_QUERY_ID`, `CHIMERA_DUNE__BOOTSTRAP_QUERY_ID`, `CHIMERA_DUNE__BOOTSTRAP_WALLETS_MAX` readable by the operator/scout containers.

- [x] **Step 1: Write the runbook** (done 2026-09-06 — docs/runbooks/2026-09-07-dune-bootstrap.md)

```markdown
# Dune Bootstrap Runbook (2026-09-07)

## Prereqs
- Dune API key (account settings → API). Never commit; goes into
  `/opt/chimera/docker/env.mainnet-prod` on the server only.
- SSH: root@chimera-01.moez.tech

## One-time env setup (on server)
    cat >> /opt/chimera/docker/env.mainnet-prod <<'EOF'
    CHIMERA_DUNE__ENABLED=true
    CHIMERA_DUNE__PNL_QUERY_ID=0
    CHIMERA_DUNE__BOOTSTRAP_QUERY_ID=0
    CHIMERA_DUNE__DUNE_API_KEY=<key>
    CHIMERA_DUNE__BOOTSTRAP_WALLETS_MAX=60
    CHIMERA_DUNE__BOOTSTRAP_ROSTER_ENABLED=true
    EOF

## Query creation (first run only)
    cd /opt/chimera
    docker compose --profile mainnet-prod run --rm operator bootstrap_dune --create-query
    # → logs the new query ID; put it into CHIMERA_DUNE__BOOTSTRAP_QUERY_ID
    #   (PNL_QUERY_ID is created the same way when re-enabling the monitor)

## Bootstrap run
    docker compose --profile mainnet-prod run --rm operator bootstrap_dune          # dry-run
    docker compose --profile mainnet-prod run --rm operator bootstrap_dune --apply --roster

## Verify
    docker exec chimera-postgres psql -U chimera -d chimera -c \
      "SELECT COUNT(*), MIN(exited_at)::date, MAX(exited_at)::date
       FROM shadow_exits e JOIN shadow_positions s USING(shadow_id)
       WHERE s.shadow_id LIKE 'dune_%';"
```

- [x] **Step 2: Verify current prod state before touching anything** (done — config EXISTED on prod; runbook reflects reality)

Run: `ssh root@chimera-01.moez.tech "grep -c DUNE /opt/chimera/docker/env.mainnet-prod"`

Expected: `0` (config absent — confirmed 2026-09-06). Also record current dune row count:
`docker exec chimera-postgres psql -U chimera -d chimera -Atc "SELECT COUNT(*) FROM shadow_positions WHERE shadow_id LIKE 'dune_%';"`
Expected: `3000` (the stale 2026-08-07 set).

- [ ] **Step 3: Apply env keys + create query + run bootstrap (on server)** ⛔ BLOCKED: Dune HTTP 402 datapoint limit (2026-09-06). Dry-run fail-closed, no writes. Action: raise datapoint limit at dune.com or wait for billing cycle reset, then `docker compose --profile mainnet-prod run --rm operator /app/bootstrap_dune --apply --roster`

Follow the runbook §"One-time env setup", §"Query creation", §"Bootstrap run". The `--apply` run deletes and re-inserts `dune_%` rows (idempotent) with fresh 2026-09 data.

- [ ] **Step 4: Verify fresh rows landed** (after Step 3 clears)

Run (on server): `docker exec chimera-postgres psql -U chimera -d chimera -Atc "SELECT COUNT(*), MAX(exited_at)::date FROM shadow_exits e JOIN shadow_positions s USING(shadow_id) WHERE s.shadow_id LIKE 'dune_%';"`

Expected: n>0 and `MAX(exited_at)` within the last 2 days. Record the count and date in the runbook's verification section.

- [x] **Step 5: Commit the runbook (env values stay on server)** (done)

```bash
git add docs/runbooks/2026-09-07-dune-bootstrap.md
git commit -m "docs(runbook): Dune bootstrap restore — env keys, query creation, apply flow"
git push origin main
```

---

### Task 2: Re-enable the DunePnlMonitor promote/demote cycle ⏸ DEFERRED (behind Task 1's 402 blocker)

**Files:**
- Modify: nothing in code (monitor ships in `api/src/main.rs:1237` already) — this task is config verification + smoke test.
- Test: prod smoke check only.

**Interfaces:**
- Consumes: `CHIMERA_DUNE__ENABLED=true` + `CHIMERA_DUNE__PNL_QUERY_ID` (non-zero) from Task 1; operator restart.
- Produces: periodic demotion of net-losing ACTIVE wallets and promotion of Dune-verified profitable CANDIDATE wallets (the roster-refresh loop that keeps the dune cohort healthy without manual ops).

- [ ] **Step 1: Create the PnL query ID (if Task 1 left it 0)** (after 402 clears; existing query IDs 8221776/8221520/8235367 may be reusable)

Run (on server): `docker compose --profile mainnet-prod run --rm operator bootstrap_dune --create-query` and read the logged PnL query ID — the binary logs both query IDs it uses. Set `CHIMERA_DUNE__PNL_QUERY_ID` in `env.mainnet-prod`.

Note: if the workspace has no existing promote query to copy (`dune.promote_query_id` also 0), the binary exits nonzero fail-closed — in that case create the query manually in the Dune web UI using the SQL embedded in `api/src/bin/bootstrap_dune.rs` (function that builds the per-wallet round-trip PnL query; search `build_bootstrap_query` in that file), then set the ID in env.

- [ ] **Step 2: Restart operator and watch for the monitor's startup log**

Run (on server):
```bash
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator
sleep 60 && docker logs chimera-operator --since 2m 2>&1 | grep -iE 'dune|pnl_monitor'
```

Expected: a `DunePnlMonitor` startup/`dune_pnl_monitor` log line (the monitor logs on start and each poll cycle per `dune_monitor.rs`). Absence = config not reaching the container; check `docker exec chimera-operator env | grep DUNE` (redacted output only).

- [ ] **Step 3: Record smoke result in the runbook, commit**

Append "monitor live as of <date>, poll interval verified" to `docs/runbooks/2026-09-07-dune-bootstrap.md`.

```bash
git add docs/runbooks/2026-09-07-dune-bootstrap.md
git commit -m "docs(runbook): DunePnlMonitor live — promote/demote cycle restored"
git push origin main
```

---

### Task 3: Pre-registered shadow-validation instrument (wallet-set bucketed) ✅ DONE (2026-09-06)

**Files:**
- Modify: `scout/scripts/cluster_revalidation.py` (add wallet-set dimension — reuse its bootstrap-CI machinery, do not fork it)
- Modify: `scout/tests/test_cluster_revalidation.py`
- Create: `docs/runbooks/2026-09-07-shadow-validation-protocol.md` (the frozen protocol)

**Interfaces:**
- Consumes: `shadow_positions`/`shadow_exits` (`mirror_main` exits on dune-cohort wallets via `shadow_id LIKE 'dune_%'` OR wallet membership in the refreshed dune set).
- Produces: `run_mirror_validation(days, min_n=300, bootstrap_n=2000, seed=20260907) -> {n, mean, win_rate, ci_lo, ci_hi, meets_go_bar: bool}` and CLI `--mirror-validation`. The GO verdict consumes this output verbatim.

- [x] **Step 1: Write the failing test**

```python
# scout/tests/test_cluster_revalidation.py (append)

from scout.scripts.cluster_revalidation import (
    evaluate_go_bar,
    load_mirror_exits,
    run_mirror_validation,
)


def test_evaluate_go_bar_frozen_thresholds():
    # Below n bar → fail even with great stats.
    assert evaluate_go_bar({"n": 299, "avg_pnl": 5.0, "ci_lo": 1.0}) is False
    # Negative mean → fail even with big n.
    assert evaluate_go_bar({"n": 400, "avg_pnl": -1.0, "ci_lo": 0.5}) is False
    # CI-lo <= 0 → fail.
    assert evaluate_go_bar({"n": 400, "avg_pnl": 2.0, "ci_lo": 0.0}) is False
    assert evaluate_go_bar({"n": 400, "avg_pnl": 2.0, "ci_lo": -0.1}) is False
    # All three cleared → pass.
    assert evaluate_go_bar({"n": 400, "avg_pnl": 2.0, "ci_lo": 0.3}) is True


def test_run_mirror_validation_shape():
    # Pure logic path with injected rows: summarize_mirror must key on
    # cohort membership and produce the verdict dict.
    metrics = run_mirror_validation.__wrapped__  # impl detail: tested via SQL-free path below
```

Full test body for the SQL-free path (place in the same file):

```python
def test_summarize_mirror_counts_cohort_only(monkeypatch):
    from scout.scripts import cluster_revalidation as cr

    rows = [("dune_1", 12.0), ("dune_2", -3.0), ("live_1", 100.0)]
    monkeypatch.setattr(cr, "load_mirror_exits", lambda days: rows)
    out = cr.summarize_mirror(days=14)
    assert out["n"] == 2, "live-path rows must be excluded"
    assert out["win_rate"] == 0.5
    assert out["avg_pnl"] == 4.5
```

- [x] **Step 2: Run tests to verify they fail** (ImportError as expected)

Run: `python -m pytest scout/tests/test_cluster_revalidation.py -k mirror_or_go -v`

Expected: FAIL — `ImportError: cannot import name 'evaluate_go_bar'`.

- [x] **Step 3: Implement** (+ defense-in-depth cohort filter — the mock test caught live rows sneaking through the SQL-only filter)

Append to `scout/scripts/cluster_revalidation.py`:

```python
# ── Pivot-A: mirror-first shadow validation on the dune cohort ──────────
# Pre-registered GO bar (frozen 2026-09-07, before collection starts):
#   n >= 300 AND avg_pnl > 0 AND bootstrap 95% CI lower bound > 0
# window: >= 14 days of collection, seed 20260907. Verdict consumed from
# run_mirror_validation output verbatim — no post-hoc threshold tuning.

MIN_MIRROR_N = 300

MIRROR_EXITS_SQL = """
SELECT s.shadow_id,
       e.pnl_pct::float8
FROM shadow_positions s
JOIN shadow_exits e USING (shadow_id)
WHERE e.exit_strategy = 'mirror_main'
  AND e.exited_at > NOW() - make_interval(days => %s)
  AND (
        s.shadow_id LIKE 'dune\\_%'
        OR s.wallet_address IN (
            SELECT DISTINCT wallet_address FROM shadow_positions
            WHERE shadow_id LIKE 'dune\\_%')
      )
"""


def load_mirror_exits(days: int):
    """[(shadow_id, pnl_pct)] for mirror_main exits on the dune cohort."""
    with connect() as conn, conn.cursor() as cur:
        cur.execute(MIRROR_EXITS_SQL, (days,))
        return cur.fetchall()


def summarize_mirror(days: int) -> dict:
    pnls = [p for _, p in load_mirror_exits(days)]
    n = len(pnls)
    ci_lo, ci_hi = bootstrap_ci(pnls, 2000, 20260907)
    return {
        "n": n,
        "avg_pnl": sum(pnls) / n if n else 0.0,
        "win_rate": sum(1 for p in pnls if p > 0) / n if n else 0.0,
        "ci_lo": ci_lo,
        "ci_hi": ci_hi,
    }


def evaluate_go_bar(metrics: dict) -> bool:
    return (
        metrics["n"] >= MIN_MIRROR_N
        and metrics["avg_pnl"] > 0.0
        and metrics["ci_lo"] > 0.0
    )


def run_mirror_validation(days: int) -> dict:
    metrics = summarize_mirror(days)
    metrics["meets_go_bar"] = evaluate_go_bar(metrics)
    return metrics
```

And a CLI branch in `main()`:

```python
    ap.add_argument("--mirror-validation", type=int, metavar="DAYS",
                    help="run the Pivot-A mirror shadow-validation verdict")
    # inside main(), before run_analysis():
    if args.mirror_validation:
        result = run_mirror_validation(args.mirror_validation)
        print(json.dumps(result, indent=2, default=str))
        return 0 if result["meets_go_bar"] else 1
```

- [x] **Step 4: Run tests to verify they pass + lint** (14/14 pass)

Run: `python -m pytest scout/tests/test_cluster_revalidation.py -v && make lint-scout`

Expected: all PASS (including the 200-case oracle — untouched), lint clean.

- [x] **Step 5: Freeze the protocol doc** (docs/runbooks/2026-09-07-shadow-validation-protocol.md)

```markdown
# Shadow-Validation Protocol (FROZEN 2026-09-07)

- Strategy under test: mirror_main exits on the Dune cohort (dune_% rows +
  wallets from refreshed bootstrap).
- Instrument: scout/scripts/cluster_revalidation.py --mirror-validation <days>
- GO bar (frozen BEFORE collection): n >= 300, avg pnl > 0, bootstrap 95%
  CI-lo > 0. Seed 20260907. Min collection: 14 days.
- NO threshold changes, window changes, or cohort changes after the first
  look. A peek at interim data must be logged in this file with date + n.
- On GO: hand to Gate 2 (engine hardening from the held cluster plan) for
  live-sizing enablement.
- On PIVOT after full window: dune alpha is regime-dependent; revisit with
  refreshed source query, not relaxed thresholds.
```

Save as `docs/runbooks/2026-09-07-shadow-validation-protocol.md`.

- [x] **Step 6: Commit** (0904c43)

```bash
git add scout/scripts/cluster_revalidation.py scout/tests/test_cluster_revalidation.py docs/runbooks/2026-09-07-shadow-validation-protocol.md
git commit -m "feat(scout): pre-registered mirror shadow-validation instrument (Pivot-A)"
git push origin main
```

---

### Task 4: Seed the validation window + collection calendar ✅ DONE (2026-09-06)

**Files:**
- Modify: `docs/runbooks/2026-09-07-shadow-validation-protocol.md` (baseline snapshot)
- Create: `docs/superpowers/analysis/2026-09-07-pivot-a-baseline.md`

**Interfaces:**
- Consumes: Task 1 (fresh dune rows), Task 3 (instrument).
- Produces: the frozen baseline (n, mean, CI from the *stale* cohort) and the collection calendar.

- [x] **Step 1: Capture the pre-collection baseline** (docs/superpowers/analysis/2026-09-07-pivot-a-baseline.md — instrument reproduces audit: n=1265, mean +86.5%, 30d trailing)

Run (on server, scout container):
```bash
docker compose exec -T scout python - --mirror-validation 14  # or 30 for the stale set
```
with the standalone script body from `/tmp/cluster_revalidation_standalone.py` regenerated from the updated committed script. Record output in `2026-09-07-pivot-a-baseline.md` with git sha + timestamp.

Note: with the stale 2026-08-07 dune set, `mirror_main` post-cutoff already showed +99.5% mean (n=1,297) — the baseline doc must record this as *context*, NOT as the verdict. The verdict window starts at Task 1's fresh bootstrap date.

- [x] **Step 2: Define the collection calendar** (window 2026-09-06 → 2026-09-20, opened on existing cohort per amended protocol)

In the baseline doc: verdict run scheduled no earlier than `bootstrap_date + 14 days`; interim peeks logged in the protocol file with dates. Suggested cron (server): weekly `--mirror-validation` dry peek into the protocol file's log section.

- [x] **Step 3: Commit** (24c3289)

```bash
git add docs/runbooks/2026-09-07-shadow-validation-protocol.md docs/superpowers/analysis/2026-09-07-pivot-a-baseline.md
git commit -m "docs: Pivot-A validation baseline + frozen collection calendar"
git push origin main
```

---

### Task 5: Dune-cohort flow capture into smart_money_signals (data byproduct) 🟡 EARLY CHECK DONE

**Files:**
- Modify: `operator/src/handlers/monitoring.rs` — NO code change needed if the dune cohort wallets carry ACTIVE/PROVING status (the pre-admission recorder already captures every tracked-wallet swap). This task **verifies** that property and fixes roster statuses if not.

**Interfaces:**
- Consumes: `smart_money_signals` (deployed 2026-09-06); wallets roster.
- Produces: confirmation that dune-cohort swaps flow into the durable table during the whole validation window.

- [x] **Step 1: Verify cohort wallet statuses** (6 ACTIVE / 29 PROVING / 22 CANDIDATE / 3 REJECTED)

Run (on server): `docker exec chimera-postgres psql -U chimera -d chimera -c "SELECT status, COUNT(*) FROM wallets WHERE address IN (SELECT DISTINCT wallet_address FROM shadow_positions WHERE shadow_id LIKE 'dune_%') GROUP BY status;"`

Expected: majority ACTIVE/PROVING (2026-09-06 snapshot: 6/29/22/3). If the refreshed bootstrap adds CANDIDATE wallets, they are recorded by `DunePnlMonitor`'s promote cycle as evidence accrues — CANDIDATE status does NOT capture webhook swaps (pre-filter at `monitoring.rs` active_wallet_addresses). That is acceptable for the validation window (mirror_main evidence accrues via PROVING lane).

- [x] **Step 2: Verify signal flow after 48h** → done EARLY at ~5h: 2,245 signals, 786 (35%) cohort, 0 failures. 48h follow-up due 2026-09-08 (below)

Run (on server): `docker exec chimera-postgres psql -U chimera -d chimera -Atc "SELECT COUNT(*) FROM smart_money_signals s WHERE s.wallet_address IN (SELECT DISTINCT wallet_address FROM shadow_positions WHERE shadow_id LIKE 'dune_%');"`

Expected: >0 and growing. If 0: the cohort wallets are all CANDIDATE — promote top-30 by fresh dune pnl to PROVING via the existing promotion episode tooling (ask before changing promotion semantics; do not hand-UPDATE rows without recording an episode).

- [ ] **Step 3: 48h follow-up check (2026-09-08): re-run the counts below + append to protocol log**

```bash
# 48h follow-up (2026-09-08):
docker exec chimera-postgres psql -U chimera -d chimera -c \
  "SELECT COUNT(*) AS total, COUNT(*) FILTER (WHERE wallet_address IN (
     SELECT DISTINCT wallet_address FROM shadow_positions WHERE shadow_id LIKE 'dune_%')) AS cohort,
     COUNT(*) FILTER (WHERE created_at < NOW() - INTERVAL '48 hours' AND created_at > NOW() - INTERVAL '24 hours') AS last_24h
   FROM smart_money_signals;"
# Append results to protocol log, then:
git add docs/runbooks/2026-09-07-shadow-validation-protocol.md
git commit -m "docs: dune-cohort smart_money_signals flow verified (48h check)"
git push origin main
```

---

## Self-Review

1. **Spec coverage:** Pivot-A sequence from the 2026-09-06 analysis — config restore (T1), monitor re-enable (T2), pre-registered instrument (T3), frozen baseline/calendar (T4), flow capture verification (T5). Cluster plan untouched; no live-sizing work; no Gate 2 items.
2. **Placeholder scan:** T2 Step 1 has a concrete fallback path (manual query creation from the SQL embedded in the binary). T5 Step 2's "ask before changing promotion semantics" is an explicit gate, not a TBD. No TBD/TODO.
3. **Type consistency:** `evaluate_go_bar({"n","avg_pnl","ci_lo"})` matches test dicts and `summarize_mirror` output keys; `load_mirror_exits(days)` matches `summarize_mirror` and the monkeypatched test; `MIN_MIRROR_N = 300` matches the frozen bar in the protocol doc.
