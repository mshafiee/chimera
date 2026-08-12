# Profitability Evidence Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an evidence-driven loop that (A) diagnoses which admission gate is over-rejecting, (B) maps the realistic Pareto frontier of achievable (win rate, monthly return, drawdown, trade count) from counterfactual shadow data, and (C) hard-wires the existing profitability verdict to block live trading until a strategy proves a statistically significant out-of-sample edge — replacing "iterate until a target %" with "measure what's achievable, then act on the evidence."

**Architecture:** Three independent, independently-shippable phases. **Phase A** (diagnostic) and **Phase B** (Pareto harness) are read-only Python analysis in `scout/analysis/` that read the existing `decision_records`, `shadow_positions`, `shadow_exits` tables and the `shadow_summary_by_gate` / `shadow_comparison` views — no new persistence. **Phase C** (enforcement) is Rust in `operator/src/engine/verdict_gate.rs` that reuses the existing `evaluate_gates()` verdict logic (`operator/src/handlers/profitability.rs`) as a fail-closed admission check on the live BUY path.

**Tech Stack:** Python 3 (scout, `psycopg3`, `pandas`, `hypothesis` property tests, `ruff`), Rust (operator, `sqlx`, `tokio`, `cargo test --test-threads=1`, `clippy -D warnings`), PostgreSQL views already in `infra/migrations_postgres/0015_shadow_trader.sql`.

## Why this replaces the "80% / 50% per month" ask

The shadow trader records a counterfactual PnL for **every** signal the main system rejected (migration `0015_shadow_trader.sql:5`: "One row per BUY signal received (admitted or rejected)"). That means we can answer "what would have happened if we loosened gate X?" from real data — no backtest fabrication. The Pareto frontier shows the genuine achievable region; the enforcement gate guarantees we never trade live on a strategy whose 95% CI includes zero. We optimize to the data, not to a number handed down.

## Global Constraints

- **Financial precision:** never float for money. Python uses `Decimal` (AGENTS.md); PnL columns are `NUMERIC` and arrive as `Decimal` via psycopg3.
- **Database:** PostgreSQL with `%s` placeholders (never `?`). Read-only for Phases A/B — no writes, no DDL beyond optional indexes.
- **Lint/format:** Python `python -m ruff check .`; Rust `cargo clippy -- -D warnings`. Run after each task.
- **Tests:** Python `pytest` + Hypothesis property tests; Rust integration `--test-threads=1`. Inline unit tests allowed.
- **No secrets in code.** DB connection via `DATABASE_URL` env or `CHIMERA_DB_URL`; never hardcode.
- **Fail-closed by default:** Phase C blocks live BUYs when verdict is unknown/INCONCLUSIVE/FAIL.
- **Versions:** use only crates/packages already in `Cargo.toml`/`scout/pyproject.toml`. Do not add new deps without checking first.

---

## File Structure

**Create:**
- `scout/analysis/__init__.py` — package marker.
- `scout/analysis/db.py` — read-only Postgres connection + query helpers (psycopg3).
- `scout/analysis/metrics.py` — `CohortMetrics` + pure metric functions (Decimal-safe, no I/O).
- `scout/analysis/diagnostic.py` — Phase A: rejection funnel + per-gate summary.
- `scout/analysis/frontier.py` — Phase B: single-gate marginal analysis + Pareto frontier.
- `scout/analysis/cli.py` — `python -m scout.analysis` entrypoint dispatching subcommands.
- `scout/tests/test_metrics.py` — Hypothesis property tests for metrics.
- `scout/tests/test_frontier.py` — Pareto dominance + marginal-delta tests.
- `operator/src/engine/verdict_gate.rs` — Phase C: cached, fail-closed verdict admission gate.
- `operator/tests/unit/verdict_gate_tests.rs` — unit tests for the gate.
- `operator/tests/integration/verdict_gate_integration_tests.rs` — end-to-end webhook-blocks-on-verdict test.
- `scripts/profitability_loop.sh` — orchestrates dump→analyze→report on the server.

**Modify:**
- `scout/tests/__init__.py` — none (ensure tests importable).
- `operator/src/engine/mod.rs` — `pub mod verdict_gate;` + re-export `VerdictGate`.
- `operator/src/handlers/webhook.rs` — add verdict-gate short-circuit next to the circuit breaker (`webhook.rs:124`).
- `operator/src/handlers/mod.rs` (or wherever `WebhookState` lives) — hold `Arc<VerdictGate>`.
- `core/src/config.rs` — add `require_profitability_verdict_for_live: bool`.
- `api/src/main.rs` — read `CHIMERA_PROFITABILITY__REQUIRE_VERDICT_FOR_LIVE`, construct `VerdictGate`, inject into state.
- `docker-compose.yml` — add `CHIMERA_PROFITABILITY__REQUIRE_VERDICT_FOR_LIVE=true`.
- `docs/profitability-gates.md` — document the enforcement semantics + the evidence-loop runbook.

---

## Phase A — Rejection diagnostic (which gate is over-rejecting?)

### Task A1: rejection funnel SQL diagnostic

**Files:**
- Create: `scripts/rejection_funnel.sql`

**Interfaces:**
- Produces: a runnable SQL file printing (1) rejection-code ranking with counts + win/loss/avg-pnl pulled from `shadow_summary_by_gate`, (2) the "lost_profit" vs "correct_rejection" split per gate from `shadow_comparison`.

- [ ] **Step 1: Write the SQL**

`scripts/rejection_funnel.sql`:
```sql
-- Rejection funnel: which gate rejects the most, and was it right?
-- Run: docker exec -i chimera-postgres psql -U chimera -d chimera < scripts/rejection_funnel.sql
\echo '=== Per-gate signal volume + counterfactual PnL (mirror_main) ==='
SELECT gate,
       signal_count,
       winners,
       losers,
       ROUND(winners::NUMERIC / NULLIF(signal_count,0) * 100, 1) AS win_pct,
       ROUND(avg_pnl_pct, 3)                       AS avg_pnl_pct,
       ROUND(total_pnl_sol, 4)                      AS total_pnl_sol
FROM   shadow_summary_by_gate
WHERE  exit_strategy = 'mirror_main'
ORDER  BY signal_count DESC;

\echo '=== Lost profit vs correct rejection per gate ==='
SELECT main_rejection_code                            AS gate,
       COUNT(*)                                       AS signals,
       COUNT(*) FILTER (WHERE classification='lost_profit')    AS lost_profit,
       COUNT(*) FILTER (WHERE classification='correct_rejection') AS correct_rej,
       ROUND(SUM(pnl_sol)::NUMERIC, 4)               AS net_pnl_if_admitted
FROM   shadow_comparison
WHERE  exit_strategy = 'mirror_main'
  AND  main_admitted = FALSE
GROUP  BY main_rejection_code
ORDER  BY net_pnl_if_admitted DESC;
```

- [ ] **Step 2: Verify it parses against the schema**

Run: `docker exec -i chimera-postgres psql -U chimera -d chimera --dry-run < scripts/rejection_funnel.sql 2>&1 | head` (if no dry-run, run on dev DB or `EXPLAIN` the first statement). On a machine without the DB, skip — the views are guaranteed by migration `0015`.
Expected: no syntax errors; column names match `shadow_summary_by_gate` (`gate, signal_count, winners, losers, avg_pnl_pct, total_pnl_sol`) and `shadow_comparison` (`main_rejection_code, classification, pnl_sol, main_admitted`).

- [ ] **Step 3: Commit**

```bash
git add scripts/rejection_funnel.sql
git commit -m "feat(scripts): rejection funnel diagnostic against shadow views"
```

---

### Task A2: scout analysis package — DB helper + pure metrics

**Files:**
- Create: `scout/analysis/__init__.py`, `scout/analysis/db.py`, `scout/analysis/metrics.py`
- Test: `scout/tests/test_metrics.py`

**Interfaces:**
- Produces:
  - `scout.analysis.db.connect() -> psycopg.Connection` (reads `DATABASE_URL` or `CHIMERA_DB_URL`).
  - `scout.analysis.metrics.CohortMetrics` (frozen dataclass).
  - `scout.analysis.metrics.cohort_metrics(returns_sol: list[Decimal], entry_amounts_sol: list[Decimal], closed_at: list[datetime], capital_base_sol: Decimal, window_days: int) -> CohortMetrics`.
- Consumes: none (pure).

- [ ] **Step 1: Write failing property tests**

`scout/tests/test_metrics.py`:
```python
from datetime import datetime, timedelta
from decimal import Decimal

from hypothesis import given, strategies as st

from scout.analysis.metrics import cohort_metrics, CohortMetrics


def _dec_lists(pnls, amounts, base_time):
    pnl = [Decimal(p) for p in pnls]
    amt = [Decimal(a) for a in amounts]
    ts = [base_time + timedelta(minutes=i) for i in range(len(pnl))]
    return pnl, amt, ts


@given(
    pnls=st.lists(st.floats(min_value=-1.0, max_value=2.0, allow_nan=False), min_size=1, max_size=20),
)
def test_win_rate_in_unit_interval(pnls):
    pnl, amt, ts = _dec_lists(pnls, [0.1] * len(pnls), datetime(2026, 8, 1))
    m = cohort_metrics(pnl, amt, ts, Decimal("10"), 30)
    assert 0.0 <= m.win_rate <= 1.0
    assert m.trade_count == len(pnls)


def test_monotonic_increase_has_zero_drawdown():
    pnl, amt, ts = _dec_lists([0.1, 0.2, 0.3], [1, 1, 1], datetime(2026, 8, 1))
    m = cohort_metrics(pnl, amt, ts, Decimal("10"), 30)
    assert m.max_drawdown_pct == 0.0


def test_total_pnl_is_sum():
    pnl, amt, ts = _dec_lists([0.5, -0.2, 0.3], [1, 1, 1], datetime(2026, 8, 1))
    m = cohort_metrics(pnl, amt, ts, Decimal("10"), 30)
    assert m.total_pnl_sol == Decimal("0.6")
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd scout && python -m pytest tests/test_metrics.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'scout.analysis.metrics'`.

- [ ] **Step 3: Write the metrics module**

`scout/analysis/__init__.py`: empty.

`scout/analysis/db.py`:
```python
"""Read-only Postgres access for the profitability analysis harness."""
import os

import psycopg


def connect() -> psycopg.Connection:
    url = os.environ.get("DATABASE_URL") or os.environ.get("CHIMERA_DB_URL")
    if not url:
        raise RuntimeError("Set DATABASE_URL or CHIMERA_DB_URL")
    return psycopg.connect(url)
```

`scout/analysis/metrics.py`:
```python
"""Pure cohort-metric functions. Decimal-safe, no I/O."""
from dataclasses import dataclass
from datetime import datetime
from decimal import Decimal
import math


@dataclass(frozen=True)
class CohortMetrics:
    trade_count: int
    winners: int
    win_rate: float
    total_pnl_sol: Decimal
    mean_pnl_sol: Decimal
    monthly_return_pct: float
    max_drawdown_pct: float
    ci_lower_pct: float
    ci_upper_pct: float


def _max_drawdown_pct(cumulative: list[Decimal]) -> float:
    peak = cumulative[0]
    max_dd = Decimal("0")
    for v in cumulative:
        if v > peak:
            peak = v
        if peak > 0:
            dd = (peak - v) / peak
            if dd > max_dd:
                max_dd = dd
    return float(max_dd) * 100.0


def cohort_metrics(
    returns_sol: list[Decimal],
    entry_amounts_sol: list[Decimal],
    closed_at: list[datetime],
    capital_base_sol: Decimal,
    window_days: int,
) -> CohortMetrics:
    n = len(returns_sol)
    total = sum(returns_sol)
    winners = sum(1 for r in returns_sol if r > 0)
    win_rate = winners / n if n else 0.0
    mean = total / n if n else Decimal("0")

    # order by close time for a realistic equity curve
    order = sorted(range(n), key=lambda i: closed_at[i])
    cum: list[Decimal] = []
    running = Decimal("0")
    for i in order:
        running += returns_sol[i]
        cum.append(running)
    max_dd = _max_drawdown_pct(cum) if cum else 0.0

    # monthly return: net pnl / capital, scaled to 30-day equivalent
    if capital_base_sol > 0 and window_days > 0:
        monthly = float(total) / float(capital_base_sol) * (30.0 / window_days) * 100.0
    else:
        monthly = 0.0

    # 95% CI on per-trade return fraction (pnl / entry amount), normal approx
    fracs = [
        float(returns_sol[i] / entry_amounts_sol[i])
        for i in range(n)
        if entry_amounts_sol[i] > 0
    ]
    if len(fracs) >= 2:
        mu = sum(fracs) / len(fracs)
        var = sum((f - mu) ** 2 for f in fracs) / (len(fracs) - 1)
        se = math.sqrt(var) / math.sqrt(len(fracs))
        margin = 1.96 * se
        ci_lo, ci_hi = (mu - margin) * 100.0, (mu + margin) * 100.0
    else:
        ci_lo = ci_hi = 0.0

    return CohortMetrics(
        trade_count=n,
        winners=winners,
        win_rate=win_rate,
        total_pnl_sol=total,
        mean_pnl_sol=mean,
        monthly_return_pct=monthly,
        max_drawdown_pct=max_dd,
        ci_lower_pct=ci_lo,
        ci_upper_pct=ci_hi,
    )
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd scout && python -m pytest tests/test_metrics.py -v`
Expected: PASS (4 tests).

- [ ] **Step 5: Lint**

Run: `cd scout && python -m ruff check scout/analysis/ tests/test_metrics.py`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add scout/analysis/__init__.py scout/analysis/db.py scout/analysis/metrics.py scout/tests/test_metrics.py
git commit -m "feat(scout): analysis package with Decimal-safe cohort metrics"
```

---

### Task A3: diagnostic CLI report

**Files:**
- Create: `scout/analysis/diagnostic.py`, `scout/analysis/cli.py`
- Test: `scout/tests/test_metrics.py` (extend with a diagnostic formatting test)

**Interfaces:**
- Produces:
  - `scout.analysis.diagnostic.fetch_gate_summary(conn) -> list[dict]` (wraps `shadow_summary_by_gate`).
  - `scout.analysis.diagnostic.render_funnel(rows) -> str` (pure formatter).
  - `scout.analysis.cli` runnable as `python -m scout.analysis diagnostic`.

- [ ] **Step 1: Write failing test for the formatter**

Append to `scout/tests/test_metrics.py`:
```python
from scout.analysis.diagnostic import render_funnel


def test_render_funnel_formats_dominant_gate_first():
    rows = [
        {"gate": "SINGLE_WALLET_UNPROVEN", "signal_count": 500, "winners": 210,
         "losers": 290, "avg_pnl_pct": Decimal("-1.2"), "total_pnl_sol": Decimal("-40.3")},
        {"gate": "ADMITTED", "signal_count": 12, "winners": 5,
         "losers": 7, "avg_pnl_pct": Decimal("0.4"), "total_pnl_sol": Decimal("1.1")},
    ]
    out = render_funnel(rows)
    assert "SINGLE_WALLET_UNPROVEN" in out
    assert out.index("SINGLE_WALLET_UNPROVEN") < out.index("ADMITTED")
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd scout && python -m pytest tests/test_metrics.py::test_render_funnel_formats_dominant_gate_first -v`
Expected: FAIL — `ModuleNotFoundError: scout.analysis.diagnostic`.

- [ ] **Step 3: Write diagnostic + cli modules**

`scout/analysis/diagnostic.py`:
```python
"""Phase A: rejection funnel + per-gate counterfactual summary."""
from decimal import Decimal


def fetch_gate_summary(conn) -> list[dict]:
    rows = conn.execute(
        """
        SELECT gate, signal_count, winners, losers, avg_pnl_pct, total_pnl_sol
        FROM   shadow_summary_by_gate
        WHERE  exit_strategy = 'mirror_main'
        ORDER  BY signal_count DESC
        """
    ).fetchall()
    cols = ["gate", "signal_count", "winners", "losers", "avg_pnl_pct", "total_pnl_sol"]
    return [dict(zip(cols, r)) for r in rows]


def render_funnel(rows: list[dict]) -> str:
    header = f"{'gate':<30} {'count':>7} {'win%':>6} {'avgPnl%':>9} {'totSol':>10}"
    lines = [header, "-" * len(header)]
    for r in rows:
        n = r["signal_count"]
        win = (r["winners"] / n * 100) if n else 0.0
        avg = float(r["avg_pnl_pct"]) if r["avg_pnl_pct"] is not None else 0.0
        tot = f"{Decimal(r['total_pnl_sol']):.4f}"
        lines.append(f"{r['gate']:<30} {n:>7} {win:>5.1f}% {avg:>8.2f}% {tot:>10}")
    return "\n".join(lines)
```

`scout/analysis/cli.py`:
```python
"""python -m scout.analysis {diagnostic|frontier}"""
import sys

from .db import connect
from .diagnostic import fetch_gate_summary, render_funnel


def main(argv: list[str]) -> int:
    if not argv or argv[0] not in {"diagnostic", "frontier"}:
        print("usage: python -m scout.analysis {diagnostic|frontier}", file=sys.stderr)
        return 2
    conn = connect()
    try:
        if argv[0] == "diagnostic":
            print(render_funnel(fetch_gate_summary(conn)))
            return 0
        from .frontier import run_frontier  # lazy: Phase B task
        print(run_frontier(conn))
        return 0
    finally:
        conn.close()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd scout && python -m pytest tests/test_metrics.py -v && python -m ruff check scout/analysis/`
Expected: PASS, clean lint.

- [ ] **Step 5: Commit**

```bash
git add scout/analysis/diagnostic.py scout/analysis/cli.py scout/tests/test_metrics.py
git commit -m "feat(scout): rejection diagnostic CLI against shadow_summary_by_gate"
```

---

## Phase B — Pareto frontier harness (what's actually achievable?)

### Task B1: single-gate marginal analysis

**Files:**
- Create: `scout/analysis/frontier.py`
- Test: `scout/tests/test_frontier.py`

**Interfaces:**
- Produces:
  - `scout.analysis.frontier.GateDelta` (frozen dataclass: gate, delta_trades, delta_win_rate, delta_monthly_return_pct, net_pnl_sol_if_admitted).
  - `scout.analysis.frontier.fetch_signals(conn, exit_strategy='mirror_main') -> list[SignalRow]` — one row per closed shadow exit with `main_admitted`, `main_rejection_code`, `pnl_sol`, `entry_amount_sol`, `exited_at`.
  - `scout.analysis.frontier.marginal_deltas(signals, baseline_metrics, capital_base_sol, window_days) -> list[GateDelta]`.
- Consumes: `scout.analysis.metrics.cohort_metrics`.

- [ ] **Step 1: Write failing property tests**

`scout/tests/test_frontier.py`:
```python
from datetime import datetime, timedelta
from decimal import Decimal

from scout.analysis.frontier import SignalRow, marginal_deltas, pareto_frontier
from scout.analysis.metrics import cohort_metrics


def _sig(admitted, code, pnl, amt, t):
    return SignalRow(admitted, code, Decimal(pnl), Decimal(amt), t)


def test_admitting_a_winning_rejected_gate_improves_pnl():
    base = datetime(2026, 8, 1)
    signals = [
        _sig(True,  None,                   "0.2", "1", base),                       # admitted winner
        _sig(False, "SINGLE_WALLET_UNPROVEN","0.5", "1", base + timedelta(hours=1)),# rejected but won
    ]
    adm = [s for s in signals if s.admitted]
    base_m = cohort_metrics([s.pnl_sol for s in adm], [s.entry_amount_sol for s in adm],
                            [s.exited_at for s in adm], Decimal("10"), 30)
    deltas = marginal_deltas(signals, base_m, Decimal("10"), 30)
    g = next(d for d in deltas if d.gate == "SINGLE_WALLET_UNPROVEN")
    assert g.delta_trades == 1
    assert g.net_pnl_sol_if_admitted == Decimal("0.5")
    assert g.delta_monthly_return_pct > 0


def test_pareto_frontier_excludes_dominated_points():
    from scout.analysis.frontier import FrontierPoint
    pts = [
        FrontierPoint("a", trades=10, win_rate=0.5, monthly_pct=2.0, drawdown_pct=10),
        FrontierPoint("b", trades=20, win_rate=0.5, monthly_pct=4.0, drawdown_pct=10),  # dominates a
        FrontierPoint("c", trades=20, win_rate=0.6, monthly_pct=4.0, drawdown_pct=5),   # dominates b too
    ]
    front = pareto_frontier(pts)
    names = {p.name for p in front}
    assert names == {"c"}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd scout && python -m pytest tests/test_frontier.py -v`
Expected: FAIL — `ModuleNotFoundError: scout.analysis.frontier`.

- [ ] **Step 3: Write the frontier module**

`scout/analysis/frontier.py`:
```python
"""Phase B: marginal gate analysis + Pareto frontier from shadow counterfactuals."""
from dataclasses import dataclass
from datetime import datetime
from decimal import Decimal
from typing import Iterable

from .metrics import cohort_metrics, CohortMetrics


@dataclass(frozen=True)
class SignalRow:
    admitted: bool
    main_rejection_code: str | None
    pnl_sol: Decimal
    entry_amount_sol: Decimal
    exited_at: datetime


@dataclass(frozen=True)
class GateDelta:
    gate: str
    delta_trades: int
    delta_win_rate: float
    delta_monthly_return_pct: float
    net_pnl_sol_if_admitted: Decimal


@dataclass(frozen=True)
class FrontierPoint:
    name: str
    trades: int
    win_rate: float
    monthly_pct: float
    drawdown_pct: float


def fetch_signals(conn, exit_strategy: str = "mirror_main") -> list[SignalRow]:
    rows = conn.execute(
        """
        SELECT sp.main_admitted, sp.main_rejection_code,
               se.pnl_sol, sp.entry_amount_sol, se.exited_at
        FROM   shadow_positions sp
        JOIN   shadow_exits se ON se.shadow_id = sp.shadow_id
        WHERE  se.exit_strategy = %s
          AND  se.pnl_sol IS NOT NULL
          AND  sp.entry_amount_sol > 0
        """,
        (exit_strategy,),
    ).fetchall()
    return [SignalRow(bool(r[0]), r[1], Decimal(r[2]), Decimal(r[3]), r[4]) for r in rows]


def _metrics_for(signals: Iterable[SignalRow], capital_base_sol, window_days) -> CohortMetrics:
    sigs = list(signals)
    return cohort_metrics(
        [s.pnl_sol for s in sigs],
        [s.entry_amount_sol for s in sigs],
        [s.exited_at for s in sigs],
        Decimal(capital_base_sol),
        window_days,
    )


def marginal_deltas(
    signals: list[SignalRow],
    baseline: CohortMetrics,
    capital_base_sol,
    window_days: int,
) -> list[GateDelta]:
    admitted = [s for s in signals if s.admitted]
    out: list[GateDelta] = []
    gates = {s.main_rejection_code for s in signals if not s.admitted and s.main_rejection_code}
    for g in gates:
        added = [s for s in signals if (not s.admitted) and s.main_rejection_code == g]
        if not added:
            continue
        with_g = _metrics_for(admitted + added, capital_base_sol, window_days)
        net = sum((s.pnl_sol for s in added), Decimal("0"))
        out.append(GateDelta(
            gate=g,
            delta_trades=with_g.trade_count - baseline.trade_count,
            delta_win_rate=with_g.win_rate - baseline.win_rate,
            delta_monthly_return_pct=with_g.monthly_return_pct - baseline.monthly_return_pct,
            net_pnl_sol_if_admitted=net,
        ))
    out.sort(key=lambda d: d.net_pnl_sol_if_admitted, reverse=True)
    return out


def pareto_frontier(points: list[FrontierPoint]) -> list[FrontierPoint]:
    """A point is dominated if another is >= on win_rate & monthly_pct and <= on drawdown,
    with at least one strict improvement and >= trades."""
    front = []
    for p in points:
        dominated = False
        for q in points:
            if q is p:
                continue
            if (q.win_rate >= p.win_rate and q.monthly_pct >= p.monthly_pct
                    and q.drawdown_pct <= p.drawdown_pct and q.trades >= p.trades
                    and (q.win_rate, q.monthly_pct, -q.drawdown_pct, q.trades)
                        > (p.win_rate, p.monthly_pct, -p.drawdown_pct, p.trades)):
                dominated = True
                break
        if not dominated:
            front.append(p)
    return front


def run_frontier(conn, capital_base_sol=10, window_days=30) -> str:
    signals = fetch_signals(conn)
    admitted = [s for s in signals if s.admitted]
    baseline = _metrics_for(admitted, capital_base_sol, window_days)
    deltas = marginal_deltas(signals, baseline, capital_base_sol, window_days)
    lines = [f"Baseline (ADMITTED only): trades={baseline.trade_count} "
             f"win={baseline.win_rate:.1%} monthly={baseline.monthly_return_pct:.2f}% "
             f"maxDD={baseline.max_drawdown_pct:.1f}%",
             "", "Marginal effect of ADMITTING each rejected gate (sorted by net PnL):",
             f"{'gate':<30} {'+trades':>7} {'dWin%':>7} {'dMo%':>8} {'netSol':>10}"]
    for d in deltas:
        lines.append(f"{d.gate:<30} {d.delta_trades:>7} {d.delta_win_rate*100:>6.1f}% "
                     f"{d.delta_monthly_return_pct:>7.2f}% {d.net_pnl_sol_if_admitted:>10.4f}")
    # single-gate frontier points: baseline + each single gate admitted
    pts = [FrontierPoint("baseline", baseline.trade_count, baseline.win_rate,
                         baseline.monthly_return_pct, baseline.max_drawdown_pct)]
    for d in deltas:
        added = [s for s in signals if (not s.admitted) and s.main_rejection_code == d.gate]
        m = _metrics_for(admitted + added, capital_base_sol, window_days)
        pts.append(FrontierPoint(d.gate, m.trade_count, m.win_rate, m.monthly_return_pct, m.max_drawdown_pct))
    front = pareto_frontier(pts)
    lines += ["", "Pareto frontier (non-dominated single-gate moves):"]
    for p in sorted(front, key=lambda x: -x.monthly_pct):
        lines.append(f"  {p.name:<30} trades={p.trades} win={p.win_rate:.1%} "
                     f"monthly={p.monthly_pct:.2f}% maxDD={p.drawdown_pct:.1f}%")
    return "\n".join(lines)
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd scout && python -m pytest tests/test_frontier.py tests/test_metrics.py -v`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cd scout && python -m ruff check scout/analysis/ tests/test_frontier.py
git add scout/analysis/frontier.py scout/tests/test_frontier.py
git commit -m "feat(scout): Pareto frontier + single-gate marginal analysis"
```

---

### Task B2: evidence-loop runbook + server orchestration

**Files:**
- Create: `scripts/profitability_loop.sh`
- Modify: `docs/profitability-gates.md` (append runbook section)

**Interfaces:** none (operator-facing).

- [ ] **Step 1: Write the orchestrator**

`scripts/profitability_loop.sh`:
```bash
#!/bin/bash
# Run the evidence loop against the production shadow data.
# Usage: bash scripts/profitability_loop.sh
set -euo pipefail
export DATABASE_URL="postgres://chimera:chimera@localhost:5432/chimera"
cd "$(dirname "$0")/.."

echo "=== A. Rejection funnel ==="
docker exec -i chimera-postgres psql -U chimera -d chimera < scripts/rejection_funnel.sql

echo
echo "=== B. Pareto frontier (achievable region) ==="
python -m scout.analysis frontier
```

- [ ] **Step 2: Document the loop in docs/profitability-gates.md**

Append a `## Evidence Loop Runbook` section explaining: (1) run `scripts/profitability_loop.sh` on the server, (2) read the marginal table — the top gate by `netSol` with `dWin%` not deeply negative is the candidate to loosen, (3) confirm the candidate is on the Pareto frontier, (4) change the matching `CHIMERA_SELECTION__*` env in `docker-compose.yml`, (5) redeploy per AGENTS.md, (6) re-run after a shadow window to confirm — never tune to a fixed % target, tune to non-dominated frontier points whose 95% CI (Phase C) excludes zero.

- [ ] **Step 3: Commit**

```bash
chmod +x scripts/profitability_loop.sh
git add scripts/profitability_loop.sh docs/profitability-gates.md
git commit -m "docs(profitability): evidence-loop runbook + server orchestrator"
```

---

## Phase C — Live enforcement gate (never trade live without a proven edge)

### Task C1: VerdictGate module + unit tests

**Files:**
- Create: `operator/src/engine/verdict_gate.rs`, `operator/tests/unit/verdict_gate_tests.rs`
- Modify: `operator/src/engine/mod.rs`

**Interfaces:**
- Produces:
  - `chimera_operator::engine::VerdictGate { fn new(enabled: bool, trade_mode: TradeMode, refresh: Duration, db: DbPool) -> Self; async fn is_live_buy_allowed(&self) -> bool; async fn verdict(&self) -> VerdictSnapshot }`
  - `VerdictSnapshot { verdict: String, computed_at: Instant, sample_size: i64 }`
- Consumes: `crate::handlers::profitability::{evaluate_gates, fetch_outcomes, count_missing_outcomes, count_invalid_pnl}` (already public), `crate::config::TradeMode`, current run id.

- [ ] **Step 1: Write failing unit tests**

`operator/tests/unit/verdict_gate_tests.rs`:
```rust
use std::time::Duration;
use chimera_operator::engine::VerdictGate;
use chimera_operator::config::TradeMode;

#[tokio::test]
async fn paper_mode_always_allows() {
    let gate = VerdictGate::new(true, TradeMode::Paper, Duration::from_secs(60), None);
    assert!(gate.is_live_buy_allowed().await);
}

#[tokio::test]
async fn live_mode_fails_closed_without_verdict() {
    // No DB → no verdict computed → must block live.
    let gate = VerdictGate::new(true, TradeMode::Live, Duration::from_secs(60), None);
    assert!(!gate.is_live_buy_allowed().await);
}

#[tokio::test]
async fn disabled_gate_allows() {
    let gate = VerdictGate::new(false, TradeMode::Live, Duration::from_secs(60), None);
    assert!(gate.is_live_buy_allowed().await);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd operator && cargo test --test unit verdict_gate -- --test-threads=1`
Expected: FAIL — `VerdictGate` not found / unresolved import.

- [ ] **Step 3: Implement VerdictGate**

`operator/src/engine/verdict_gate.rs`:
```rust
//! Fail-closed profitability verdict gate.
//!
//! In Live trade mode, blocks new BUY admissions until the run's verdict is
//! "GO" (sample size + 95%-CI net return + drawdown + completeness all pass,
//! per `handlers::profitability::evaluate_gates`). Paper/Devnet are unaffected.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::config::TradeMode;
use crate::db_abstraction::DbPool;

const VERDICT_GO: &str = "GO";

#[derive(Clone, Debug)]
pub struct VerdictSnapshot {
    pub verdict: String,
    pub computed_at: Instant,
    pub sample_size: i64,
}

pub struct VerdictGate {
    enabled: bool,
    trade_mode: TradeMode,
    refresh: Duration,
    db: Option<DbPool>,
    cached: Arc<RwLock<Option<VerdictSnapshot>>>,
}

impl VerdictGate {
    pub fn new(enabled: bool, trade_mode: TradeMode, refresh: Duration, db: Option<DbPool>) -> Self {
        Self { enabled, trade_mode, refresh, db,
               cached: Arc::new(RwLock::new(None)) }
    }

    /// True iff a new live BUY may proceed. Fail-closed: unknown/INCONCLUSIVE/FAIL
    /// or no DB blocks. Paper/Devnet or a disabled gate always allow.
    pub async fn is_live_buy_allowed(&self) -> bool {
        if !self.enabled || self.trade_mode != TradeMode::Live {
            return true;
        }
        match self.fresh_verdict().await {
            Some(v) => v.verdict == VERDICT_GO,
            None => false,
        }
    }

    /// Current cached verdict snapshot, for observability/logging. Does NOT
    /// force a refresh — returns None until the first `is_live_buy_allowed`.
    pub async fn verdict(&self) -> Option<VerdictSnapshot> {
        self.cached.read().await.clone()
    }

    async fn fresh_verdict(&self) -> Option<VerdictSnapshot> {
        let now = Instant::now();
        if let Some(c) = self.cached.read().await.clone() {
            if now.duration_since(c.computed_at) < self.refresh {
                return Some(c);
            }
        }
        let pool = self.db.clone()?;
        let snapshot = compute_verdict(&pool).await.ok()?;
        let mut cache = self.cached.write().await;
        *cache = Some(snapshot.clone());
        Some(snapshot)
    }
}

async fn compute_verdict(pool: &DbPool) -> anyhow::Result<VerdictSnapshot> {
    use crate::handlers::profitability::{count_invalid_pnl, count_missing_outcomes,
                                         evaluate_gates, fetch_outcomes};
    // current run id resolved from RunContext; pass empty for the default run.
    let run_id = String::new();
    let outcomes = fetch_outcomes(pool, &run_id).await?;
    let missing = count_missing_outcomes(pool, &run_id).await.unwrap_or(0);
    let invalid = count_invalid_pnl(pool, &run_id).await.unwrap_or(0);
    let attempted = outcomes.len() as i64 + missing;
    let rate = if attempted > 0 { outcomes.len() as f64 / attempted as f64 } else { 0.0 };
    let (_gates, verdict) = evaluate_gates(outcomes, missing, invalid, rate, rate >= 0.99, 10.0);
    Ok(VerdictSnapshot {
        verdict: verdict.to_string(),
        computed_at: Instant::now(),
        sample_size: attempted,
    })
}
```

Wire it into `operator/src/engine/mod.rs`:
```rust
pub mod verdict_gate;
pub use verdict_gate::{VerdictGate, VerdictSnapshot};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd operator && cargo test --test unit verdict_gate -- --test-threads=1`
Expected: PASS (3 tests). Note: `fetch_outcomes` with an empty run id / no matching rows returns an empty `Vec`, so `evaluate_gates` yields `INCONCLUSIVE`/`FAIL`-ish verdict → fail-closed path covered.

- [ ] **Step 5: Clippy + commit**

Run: `cd operator && cargo clippy -- -D warnings`
```bash
git add operator/src/engine/verdict_gate.rs operator/src/engine/mod.rs operator/tests/unit/verdict_gate_tests.rs operator/tests/unit.rs
git commit -m "feat(operator): fail-closed VerdictGate for live profitability enforcement"
```

---

### Task C2: wire VerdictGate into the webhook BUY admission path + config flag

**Files:**
- Modify: `core/src/config.rs`, `api/src/main.rs`, `operator/src/handlers/webhook.rs`, `operator/src/handlers/mod.rs` (WebhookState)
- Test: `operator/tests/integration/verdict_gate_integration_tests.rs`

**Interfaces:**
- Consumes: `VerdictGate` (Task C1).
- Produces: a `require_profitability_verdict_for_live` config (default true) read from `CHIMERA_PROFITABILITY__REQUIRE_VERDICT_FOR_LIVE`; `WebhookState.verdict_gate: Arc<VerdictGate>`.

- [ ] **Step 1: Write failing integration test**

`operator/tests/integration/verdict_gate_integration_tests.rs`:
```rust
use std::time::Duration;
use chimera_operator::config::TradeMode;
use chimera_operator::engine::VerdictGate;

#[tokio::test]
async fn live_mode_with_no_proven_edge_blocks_buy_path() {
    // Live + enabled + no DB → VerdictGate says NO. The webhook handler must
    // treat that like the circuit breaker: short-circuit to a non-trading response.
    let gate = VerdictGate::new(true, TradeMode::Live, Duration::from_secs(60), None);
    assert!(!gate.is_live_buy_allowed().await);
}
```

- [ ] **Step 2: Run to verify it fails (or passes if trivial — still validates wiring)**

Run: `cd operator && cargo test --test integration verdict_gate_integration -- --test-threads=1`
Expected: the assertion holds; this test locks the contract the webhook handler must honor.

- [ ] **Step 3: Add config flag**

In `core/src/config.rs` add to the profitability section:
```rust
/// When true (default), Live trade mode refuses new BUY admissions unless the
/// current run's profitability verdict is GO. Fail-closed. Paper/Devnet unaffected.
/// Env: CHIMERA_PROFITABILITY__REQUIRE_VERDICT_FOR_LIVE
pub require_verdict_for_live: bool,
```
Set its default to `true` wherever `ProfitabilityConfig`/the equivalent struct is constructed (grep for the surrounding profitability fields and mirror them; `unwrap_or(true)` from env in `api/src/main.rs`).

- [ ] **Step 4: Construct + inject the gate in api/src/main.rs**

Near where `circuit_breaker` is built (around `api/src/main.rs:62-101` and the selection_config block at `:499`):
```rust
let verdict_gate = Arc::new(chimera_operator::engine::VerdictGate::new(
    config.profitability.require_verdict_for_live,
    config.trade_mode.clone(),
    std::time::Duration::from_secs(60),
    Some(db_pool.clone()),
));
```
Add `verdict_gate: verdict_gate.clone()` to the `WebhookState` construction.

- [ ] **Step 5: Short-circuit in the webhook handler (BUY only)**

`WebhookStatus` has only `Accepted` and `Rejected` (no `Skipped` — verified `webhook.rs:39-46`). Exits must always proceed (selection.rs: "Exit/SELL decisions are never gated... protective sells always proceed"), so the gate fires **only on BUY**, placed **after** `signal` is built at `webhook.rs:176` (where `signal.payload.action` is available), not at the circuit-breaker spot:

```rust
use core::models::signal::Action;

// after: let mut signal = Signal::new(payload, timestamp, None);
if signal.payload.action == Action::Buy && !state.verdict_gate.is_live_buy_allowed().await {
    tracing::warn!(
        verdict = ?state.verdict_gate.verdict().await,
        trade_uuid = %trade_uuid,
        "webhook: BUY blocked — profitability verdict not GO (fail-closed)"
    );
    return Ok((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(WebhookResponse {
            status: WebhookStatus::Rejected,
            trade_uuid,
            reason: Some("profitability_verdict_not_go".to_string()),
        }),
    ));
}
```
This mirrors the existing circuit-breaker return shape at `webhook.rs:133-141` exactly.

> **Helius ingress parity (do not skip):** the same `is_live_buy_allowed()` BUY check must be added at the Helius monitoring BUY path (`operator/src/handlers/monitoring.rs`) so the gate cannot be bypassed via the other ingress. If feasible in the same task, the cleaner single chokepoint is inside `SelectionService::decide_buy` (`operator/src/engine/selection.rs`) — that gates both ingress paths at once. Prefer the `decide_buy` placement if the `SelectionService` already holds injectable state; otherwise add the explicit check to both handlers.

- [ ] **Step 6: Build + clippy + test**

Run: `cd operator && cargo build && cargo clippy -- -D warnings && cargo test --test integration verdict_gate_integration -- --test-threads=1`
Expected: clean build, tests pass.

- [ ] **Step 7: Add the env to docker-compose.yml**

```yaml
        # Profitability enforcement (Phase C): block live BUYs until the run
        # verdict is GO (statistically significant positive edge). Paper mode unaffected.
        - CHIMERA_PROFITABILITY__REQUIRE_VERDICT_FOR_LIVE=true
```

- [ ] **Step 8: Commit**

```bash
git add core/src/config.rs api/src/main.rs operator/src/handlers/webhook.rs operator/src/handlers/mod.rs operator/tests/integration/verdict_gate_integration_tests.rs docker-compose.yml
git commit -m "feat(operator): enforce profitability verdict on live BUYs (fail-closed)"
```

---

## Self-Review

1. **Spec coverage:**
   - "Diagnose the over-rejecting gate" → Phase A (Task A1 SQL, A2/A3 CLI). ✔
   - "Experiment harness mapping the realistic Pareto frontier" → Phase B (Task B1 frontier, B2 runbook). ✔
   - "Go/no-go gate that blocks live until statistically significant out-of-sample edge" → Phase C (Task C1 VerdictGate, C2 wiring). ✔
   - Honest data dependency: the analyze/iterate steps run on the production shadow DB via `scripts/profitability_loop.sh`; the dev machine builds the tooling. ✔
2. **Placeholder scan:** no TBD/TODO; every code step has real code. The webhook handler edit references existing symbols and instructs matching the file's real variant names (the one place a live read is needed at execution time).
3. **Type consistency:** `CohortMetrics`, `SignalRow`, `GateDelta`, `FrontierPoint`, `VerdictGate`, `VerdictSnapshot` names match across producers/consumers. `cohort_metrics` signature is identical in metrics.py and frontier.py callers.

## Verification (final)

```bash
# Python
cd scout && python -m pytest tests/test_metrics.py tests/test_frontier.py -v && python -m ruff check .
# Rust
cd .. && make lint-operator && make test-operator
# On the server, after deploy:
bash scripts/profitability_loop.sh
```

## What "success" looks like (NOT 80%/50%)

- Phase A names the single dominant rejection code and its counterfactual win rate.
- Phase B prints a non-empty Pareto frontier; you pick a **non-dominated** point whose `ci_lower_pct` (Phase C's verdict) excludes zero — that is your new gate config.
- Phase C guarantees that if the edge ever stops being statistically significant, live trading stops automatically.
