#!/usr/bin/env python3
"""
Validate WQS predictiveness against actual copy PnL from existing wallet data.

Reads chimera.db and answers the fundamental question:
  "Does WQS predict copy-trade profitability better than random?"

Usage:
    python scripts/validate_wqs_predictiveness.py
    python scripts/validate_wqs_predictiveness.py --db-path /path/to/chimera.db
"""

import argparse
import math
import os
import sqlite3
from pathlib import Path


def pearson(xs: list, ys: list) -> float:
    """Pearson correlation coefficient."""
    n = len(xs)
    if n < 3:
        return 0.0
    mean_x = sum(xs) / n
    mean_y = sum(ys) / n
    cov = sum((x - mean_x) * (y - mean_y) for x, y in zip(xs, ys))
    var_x = sum((x - mean_x) ** 2 for x in xs)
    var_y = sum((y - mean_y) ** 2 for y in ys)
    denom = math.sqrt(var_x * var_y)
    return cov / denom if denom > 0 else 0.0


def binomial_p_value(k: int, n: int, p: float = 0.5) -> float:
    """One-tailed binomial test: prob of >= k successes in n trials with prob p."""
    if k >= n:
        return 1.0
    from math import comb
    return sum(comb(n, i) * (p ** i) * ((1 - p) ** (n - i)) for i in range(k, n + 1))


def main():
    parser = argparse.ArgumentParser(description="Validate WQS predictiveness")
    parser.add_argument("--db-path", default=None, help="Path to chimera.db")
    parser.add_argument("--min-trades", type=int, default=5,
                        help="Minimum copy trades for a wallet to be included")
    args = parser.parse_args()

    db_path = args.db_path or os.getenv("CHIMERA_DB_PATH", "../data/chimera.db")
    if not Path(db_path).exists():
        print(f"Database not found: {db_path}")
        return

    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row

    rows = conn.execute("""
        SELECT wqs_score, realized_pnl_30d_sol, roi_30d, archetype, trade_count_30d
        FROM wallets
        WHERE realized_pnl_30d_sol IS NOT NULL
          AND trade_count_30d >= ?
          AND status IN ('ACTIVE', 'CANDIDATE')
        ORDER BY wqs_score ASC
    """, (args.min_trades,)).fetchall()
    conn.close()

    if not rows:
        print("No wallets with PnL data found in the database.")
        return

    wqs_vals = [r["wqs_score"] for r in rows]
    pnl_vals = [r["realized_pnl_30d_sol"] for r in rows]
    roi_vals = [r["roi_30d"] or 0.0 for r in rows]
    total = len(rows)
    profitable = sum(1 for p in pnl_vals if p > 0)

    # Correlations
    wqs_r = pearson(wqs_vals, pnl_vals)
    roi_r = pearson(roi_vals, pnl_vals)

    # Profitability rate test
    profit_rate = profitable / total * 100
    p_value = binomial_p_value(profitable, total)

    # Top vs bottom tercile
    sorted_pairs = sorted(zip(wqs_vals, pnl_vals, roi_vals, rows),
                          key=lambda x: x[0])
    n_tercile = total // 3
    top_tercile = sorted_pairs[-n_tercile:] if n_tercile > 0 else sorted_pairs
    bottom_tercile = sorted_pairs[:n_tercile] if n_tercile > 0 else sorted_pairs
    top_mean_pnl = sum(p for _, p, _, _ in top_tercile) / max(1, len(top_tercile))
    bottom_mean_pnl = sum(p for _, p, _, _ in bottom_tercile) / max(1, len(bottom_tercile))

    # Archetype breakdown
    archetype_stats = {}
    for r in rows:
        arch = r["archetype"] or "UNKNOWN"
        if arch not in archetype_stats:
            archetype_stats[arch] = {"count": 0, "profitable": 0, "total_pnl": 0.0}
        archetype_stats[arch]["count"] += 1
        if r["realized_pnl_30d_sol"] > 0:
            archetype_stats[arch]["profitable"] += 1
        archetype_stats[arch]["total_pnl"] += r["realized_pnl_30d_sol"]

    # Output
    print("\n" + "=" * 65)
    print("     WQS PREDICTIVENESS REPORT")
    print("=" * 65)
    print(f"  Dataset: {total} wallets with ≥{args.min_trades} copy trades\n")

    print("  WQS vs actual PnL (Pearson r):")
    print(f"    WQS correlation:  r = {wqs_r:+.4f}")
    print(f"    ROI-30d alone:    r = {roi_r:+.4f}  (baseline)")
    print()

    print("  Profitability comparison:")
    print(f"    WQS-promoted wallets profitable:  {profitable}/{total}  ({profit_rate:.1f}%)")
    print(f"    Random expectation (coin flip):    {total//2}/{total}  (50.0%)")
    print(f"    Excess over random:               {profit_rate - 50:+.1f} percentage points")
    print(f"    Binomial test p-value:            {p_value:.4f}  "
          + ("(significant)" if p_value < 0.05 else "(not significant)"))
    print()

    print("  Top vs bottom tercile (by WQS):")
    if n_tercile > 0:
        print(f"    Top third mean PnL:    {top_mean_pnl:+.4f} SOL")
        print(f"    Bottom third mean PnL: {bottom_mean_pnl:+.4f} SOL")
        print(f"    Long-short spread:     {top_mean_pnl - bottom_mean_pnl:+.4f} SOL")
    print()

    print("  By archetype:")
    for arch in sorted(archetype_stats.keys()):
        s = archetype_stats[arch]
        pct = s["profitable"] / s["count"] * 100 if s["count"] else 0
        print(f"    {arch:<10s}  {s['count']:>3d} wallets,  "
              f"{pct:5.1f}% profitable,  mean PnL {s['total_pnl']/s['count']:+.3f} SOL")
    print()

    # Verdict
    print("  VERDICT:")
    beats_random = profit_rate > 50 and p_value < 0.05
    if beats_random and wqs_r > 0.1:
        print("    WQS POSITIVELY PREDICTS profitability.")
        print("    Correlation is significant and exceeds random baseline.")
    elif wqs_r > 0:
        print("    WQS weakly predicts profitability (not statistically significant).")
        print("    More data is needed to confirm.")
    elif wqs_r < -0.1:
        print("    WQS is ANTI-PREDICTIVE (negative correlation).")
        print("    High WQS wallets tend to underperform. Investigation required.")
    else:
        print("    WQS does NOT predict profitability from available data.")
        print("    Correlation is near zero.")
    print("=" * 65 + "\n")


if __name__ == "__main__":
    main()
