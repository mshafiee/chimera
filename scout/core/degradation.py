"""
Performance degradation detection for wallets.

Checks whether an ACTIVE wallet's recent performance has declined enough
to warrant demotion to CANDIDATE status.
"""

import logging
from datetime import datetime, timedelta

from .utils import utcnow

logger = logging.getLogger(__name__)


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
                else:
                    last_trade_dt = last_trade_dt.replace(tzinfo=None) if now.tzinfo is None else last_trade_dt
                # Compare full durations, not whole-day truncation, so a trade
                # 7 days and 23 hours old is treated as "more than 7 days ago".
                if (now - last_trade_dt) > timedelta(days=7):
                    return True
            except (ValueError, TypeError) as e:
                logger.warning("Degradation check: bad last_trade_at value %r: %s", last_trade, e)

        if seven_d_roi < -15.0:
            return True

    return False
