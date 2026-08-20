"""
Fidelity of a reconstructed price path vs an external OHLCV provider (Phase 2B).

Before a reconstructed path is trusted for exit-parameter tuning, we quantify
how well it agrees with a provider candle series at matched timestamps. Only
positions whose path clears the fidelity gate are used in the grid-search.

Pure functions, no I/O, unit-tested.
"""

from __future__ import annotations

import math
from typing import Optional, Sequence, Tuple


def pearson_corr(xs: Sequence[float], ys: Sequence[float]) -> Optional[float]:
    """Pearson correlation coefficient, or None when under-sampled/constant."""
    n = len(xs)
    if n < 3 or n != len(ys):
        return None
    mx = sum(xs) / n
    my = sum(ys) / n
    cov = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    var_x = sum((x - mx) ** 2 for x in xs)
    var_y = sum((y - my) ** 2 for y in ys)
    if var_x <= 0 or var_y <= 0:
        return None
    corr = cov / math.sqrt(var_x * var_y)
    # guard against tiny floating error pushing slightly past [-1,1]
    return max(-1.0, min(1.0, corr))


def mape(reconstructed: Sequence[float], reference: Sequence[float]) -> Optional[float]:
    """Mean absolute percentage error, or None when reference has zeros."""
    diffs = []
    for r, ref in zip(reconstructed, reference):
        if ref == 0:
            return None
        diffs.append(abs(r - ref) / abs(ref))
    if not diffs:
        return None
    return sum(diffs) / len(diffs)


def fidelity(
    reconstructed: Sequence[float],
    reference: Sequence[float],
    min_corr: float = 0.9,
    max_mape: float = 0.20,
) -> Tuple[Optional[float], Optional[float], bool]:
    """Return (pearson, mape, pass). Pass requires >=3 matched points, corr
    above `min_corr`, and a defined MAPE at/below `max_mape`."""
    corr = pearson_corr(reconstructed, reference)
    m = mape(reconstructed, reference)
    n = min(len(reconstructed), len(reference))
    ok = n >= 3 and corr is not None and corr >= min_corr and m is not None and m <= max_mape
    return corr, m, ok
