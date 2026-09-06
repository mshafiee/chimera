"""Gate 1: pre-registered re-validation of the smart-money cluster hypothesis.

Replaces the unreproducible §1.2 table of the held cluster-accumulation plan
(docs/superpowers/plans/2026-09-06-smart-money-cluster-accumulation.md).
Cluster attribution is a per-position self-join over the shadow book: a
position's cluster size is the number of DISTINCT tracked wallets that
entered the same token within the trailing window before (and including)
its own entry, optionally filtered by the temporal-dispersion rule.

Pre-registered defaults (do not tune after seeing results):
    window_hours=12, min_gap_s=5, min_span_s=180, bootstrap_n=2000,
    seed=20260906

Verdict rule (pre-registered): a strategy's 3+ dispersion-valid bucket is
GO-evidence iff n >= 300 AND avg pnl_pct > 0 AND bootstrap 95% CI lower
bound > 0. If no wallet_sell/fixed_24h/fixed_4h 3+ bucket meets the rule,
the outcome is PIVOT.

Read-only: SELECTs against shadow_positions / shadow_exits only. Connection
comes from scout/analysis/db.py (DATABASE_URL or CHIMERA_DB_URL).

Precision note: pnl_pct arrives as NUMERIC and is cast to float8 here for
statistical aggregation (means/percentiles over ~10K samples). Float is the
correct tool for aggregate statistics; no money sizing derives from this
output, satisfying the repo financial-precision rule.
"""

from __future__ import annotations

import argparse
import datetime as dt
import itertools
import json
import random
import sys
from collections import defaultdict

from scout.analysis.db import connect

MIN_GAP_S = 5
MIN_SPAN_S = 180
STRATEGIES = ("wallet_sell", "fixed_24h", "fixed_4h")

# Pre-registered verdict thresholds — frozen before the run.
MIN_BUCKET_N = 300
MIN_AVG_PNL_PCT = 0.0

ATTRIBUTION_SQL = """
WITH tracked AS (
    SELECT s.shadow_id,
           s.wallet_address,
           s.token_address,
           s.opened_at
    FROM shadow_positions s
    WHERE s.opened_at > NOW() - make_interval(days => %s)
      AND s.strategy = %s
),
arrivals AS (
    SELECT t.shadow_id, o.wallet_address, MIN(o.opened_at) AS arrival
    FROM tracked t
    JOIN tracked o
      ON o.token_address = t.token_address
     AND o.opened_at BETWEEN t.opened_at - make_interval(hours => %s)
                         AND t.opened_at
    GROUP BY t.shadow_id, o.wallet_address
)
SELECT a.shadow_id,
       COUNT(DISTINCT a.wallet_address) AS cluster_size,
       ARRAY_AGG(a.arrival ORDER BY a.arrival) AS arrivals
FROM arrivals a
GROUP BY a.shadow_id
"""

EXITS_SQL = """
SELECT COALESCE(pnl_pct, 0)::float8
FROM shadow_exits
WHERE shadow_id = %s
"""


def passes_dispersion(arrivals, min_gap_s: int, min_span_s: int) -> bool:
    """True iff some i<j<k among sorted arrivals has span & both gaps in bounds.

    O(N^3) over N <= a few dozen arrivals per token-window — trivial, and
    matches the brute-force definition exactly (verified by the hypothesis
    test against an oracle).
    """
    secs = sorted(int(a.timestamp()) for a in arrivals)
    return any(
        c - a >= min_span_s and b - a >= min_gap_s and c - b >= min_gap_s
        for a, b, c in itertools.combinations(secs, 3)
    )


def bucket_for_count(count):
    if count is None:
        return "unattributed"
    if count <= 1:
        return "1"
    if count == 2:
        return "2"
    return "3+"


def bootstrap_ci(values, n_boot: int, seed: int, confidence: float = 0.95):
    """Percentile bootstrap CI of the mean. Deterministic under seed."""
    n = len(values)
    if n == 0:
        return (0.0, 0.0)
    rng = random.Random(seed)
    means = sorted(
        sum(rng.choice(values) for _ in range(n)) / n for _ in range(n_boot)
    )
    alpha = (1.0 - confidence) / 2.0
    idx_lo = int(alpha * n_boot)
    idx_hi = min(n_boot - 1, int((1.0 - alpha) * n_boot))
    return (means[idx_lo], means[idx_hi])


def load_attributions(days: int, window_hours: int, strategy: str):
    """[(shadow_id, cluster_size, [arrival,...])] for one strategy."""
    with connect() as conn, conn.cursor() as cur:
        cur.execute(ATTRIBUTION_SQL, (days, strategy, window_hours))
        return cur.fetchall()


def summarize(rows, apply_dispersion: bool, min_gap_s: int, min_span_s: int,
              bootstrap_n: int, seed: int) -> dict:
    """bucket -> {n, win_rate, avg_pnl, ci_lo, ci_hi} over joined exits."""
    pnl_by_bucket: dict[str, list[float]] = defaultdict(list)
    with connect() as conn, conn.cursor() as cur:
        for shadow_id, size, arrivals in rows:
            if apply_dispersion and (
                size < 3 or not passes_dispersion(arrivals, min_gap_s, min_span_s)
            ):
                continue
            cur.execute(EXITS_SQL, (shadow_id,))
            pnl_by_bucket[bucket_for_count(size)].extend(r[0] for r in cur.fetchall())

    out = {}
    for bucket, pnls in pnl_by_bucket.items():
        n = len(pnls)
        ci_lo, ci_hi = bootstrap_ci(pnls, bootstrap_n, seed)
        out[bucket] = {
            "n": n,
            "win_rate": sum(1 for p in pnls if p > 0) / n if n else 0.0,
            "avg_pnl": sum(pnls) / n if n else 0.0,
            "ci_lo": ci_lo,
            "ci_hi": ci_hi,
        }
    return out


def run_analysis(days: int, window_hours: int, min_gap_s: int, min_span_s: int,
                 bootstrap_n: int, seed: int) -> dict:
    """Full verdict structure: {strategy: {all|dispersion: {bucket: metrics}}}."""
    results = {}
    for strategy in STRATEGIES:
        rows = load_attributions(days, window_hours, strategy)
        results[strategy] = {
            "all": summarize(rows, False, min_gap_s, min_span_s, bootstrap_n, seed),
            "dispersion": summarize(rows, True, min_gap_s, min_span_s,
                                    bootstrap_n, seed),
        }
    return results


def verdict_for(results: dict) -> tuple[bool, str | None]:
    """(go_evidence, strategy_that_passed). Pre-registered rule."""
    for strategy, by_mode in results.items():
        cluster = by_mode["dispersion"].get("3+")
        if cluster is None:
            continue
        if (cluster["n"] >= MIN_BUCKET_N
                and cluster["avg_pnl"] > MIN_AVG_PNL_PCT
                and cluster["ci_lo"] > 0.0):
            return True, strategy
    return False, None


def render_markdown(results: dict, days: int, window_hours: int) -> str:
    lines = [
        f"# Cluster re-validation ({days}d, {window_hours}h window)\n",
        "| strategy | mode | bucket | n | win% | avg pnl | CI95 |",
        "|---|---|---|---|---|---|---|",
    ]
    for strategy, by_mode in results.items():
        for mode, buckets in by_mode.items():
            for bucket in ("1", "2", "3+"):
                m = buckets.get(bucket)
                if m is None:
                    continue
                lines.append(
                    f"| {strategy} | {mode} | {bucket} | {m['n']} "
                    f"| {m['win_rate']:.1%} | {m['avg_pnl']:+.2f} "
                    f"| [{m['ci_lo']:+.2f}, {m['ci_hi']:+.2f}] |"
                )
    return "\n".join(lines) + "\n"


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--days", type=int, default=30)
    ap.add_argument("--window-hours", type=int, default=12)
    ap.add_argument("--min-gap-s", type=int, default=MIN_GAP_S)
    ap.add_argument("--min-span-s", type=int, default=MIN_SPAN_S)
    ap.add_argument("--bootstrap-n", type=int, default=2000)
    ap.add_argument("--seed", type=int, default=20260906)
    ap.add_argument("--json", action="store_true", help="emit JSON instead of markdown")
    args = ap.parse_args(argv)

    results = run_analysis(args.days, args.window_hours, args.min_gap_s,
                           args.min_span_s, args.bootstrap_n, args.seed)

    if args.json:
        print(json.dumps(results, indent=2, default=str))
        return 0

    print(render_markdown(results, args.days, args.window_hours))
    go, strategy = verdict_for(results)
    print("## Verdict\n")
    if go:
        print(f"GO-EVIDENCE: {strategy} 3+ dispersion-valid bucket cleared the "
              "pre-registered bar. Proceed to Gate 2 review.")
        return 0
    print("PIVOT: no 3+ cluster bucket cleared the pre-registered bar. "
          "The cluster-accumulation engine stays HELD; strategy pivot review "
          "required (e.g. solo high-conviction swingers / dune_wallet revival).")
    return 1


if __name__ == "__main__":
    sys.exit(main())
