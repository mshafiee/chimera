"""Phase A: rejection funnel + per-gate counterfactual summary.

Leans on the `shadow_summary_by_gate` view (migration 0015_shadow_trader.sql),
which already aggregates signal volume, winners/losers, avg pnl and total pnl
per gate (or ADMITTED) under each exit strategy.
"""

from decimal import Decimal


def fetch_gate_summary(conn) -> list[dict]:
    """Return per-gate aggregates under mirror_main, busiest gate first."""
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
    """Pure formatter for the per-gate funnel. Busiest gate first."""
    header = f"{'gate':<30} {'count':>7} {'win%':>7} {'avgPnl%':>9} {'totSol':>12}"
    lines = [header, "-" * len(header)]
    for r in rows:
        n = r["signal_count"]
        win = (r["winners"] / n * 100.0) if n else 0.0
        avg = float(r["avg_pnl_pct"]) if r["avg_pnl_pct"] is not None else 0.0
        tot = f"{Decimal(r['total_pnl_sol']):.4f}" if r["total_pnl_sol"] is not None else "0.0000"
        lines.append(f"{r['gate']:<30} {n:>7} {win:>6.1f}% {avg:>8.2f}% {tot:>12}")
    return "\n".join(lines)
