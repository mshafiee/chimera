"""
Performance degradation detection for wallets.

Checks whether an ACTIVE wallet's recent performance has declined enough
to warrant demotion to CANDIDATE status.
"""

from datetime import datetime

from .utils import utcnow


def check_performance_degradation(metrics) -> bool:
    """
    Detect when a previously-ACTIVE wallet's recent performance has degraded.

    Returns True if:
    - 7d ROI is negative AND last trade was > 7 days ago (stale + negative trend)
    - 7d ROI is significantly negative (< -15%) regardless of recency (sharp decline)
    """
    seven_d_roi = metrics.roi_7d
    last_trade = metrics.last_trade_at

    if seven_d_roi is not None and seven_d_roi < 0:
        if last_trade:
            try:
                last_trade_dt = datetime.fromisoformat(last_trade.replace("Z", "+00:00"))
                now = utcnow()
                if last_trade_dt.tzinfo is None:
                    now = now.replace(tzinfo=None)
                days_since = (now - last_trade_dt).days
                if days_since > 7:
                    return True
            except (ValueError, TypeError):
                pass

        if seven_d_roi < -15.0:
            return True

    return False
