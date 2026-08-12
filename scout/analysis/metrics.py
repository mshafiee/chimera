"""Pure cohort-metric functions. Decimal-safe, no I/O.

These mirror the statistical intent of the Rust profitability verdict
(operator/src/handlers/profitability.rs): per-trade return fractions, a 95%
normal-approx confidence interval, win rate, peak-to-trough drawdown, and a
monthly-equivalent return. Kept in Python so the offline harness can sweep gate
configurations without round-tripping through the operator process.
"""

import math
from dataclasses import dataclass
from datetime import datetime
from decimal import Decimal


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
    """Peak-to-trough drawdown of an ordered cumulative-PnL curve, as a percent."""
    peak = cumulative[0]
    max_dd = Decimal("0")
    for value in cumulative:
        if value > peak:
            peak = value
        if peak > 0:
            dd = (peak - value) / peak
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
    """Compute the cohort metrics for one set of closed trades.

    Args:
        returns_sol: net PnL per trade in SOL (NUMERIC → Decimal).
        entry_amounts_sol: deployed SOL per trade (denominator for per-trade return).
        closed_at: realization time per trade (orders the equity curve).
        capital_base_sol: total trading capital in SOL (monthly-return denominator).
        window_days: span of the window, for 30-day-equivalent scaling.
    """
    n = len(returns_sol)
    total = sum(returns_sol, Decimal("0"))
    winners = sum(1 for r in returns_sol if r > 0)
    win_rate = winners / n if n else 0.0
    mean = total / n if n else Decimal("0")

    # Order by realization time so the equity curve reflects when PnL landed.
    order = sorted(range(n), key=lambda i: closed_at[i])
    running = Decimal("0")
    cumulative: list[Decimal] = []
    for i in order:
        running += returns_sol[i]
        cumulative.append(running)
    max_dd = _max_drawdown_pct(cumulative) if cumulative else 0.0

    # Monthly-equivalent return: net PnL / capital, scaled to 30 days.
    if capital_base_sol > 0 and window_days > 0:
        monthly = float(total) / float(capital_base_sol) * (30.0 / window_days) * 100.0
    else:
        monthly = 0.0

    # 95% CI on per-trade return fraction (pnl / entry amount), normal approx.
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
