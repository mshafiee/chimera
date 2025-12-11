"""
Wallet Quality Score (WQS) Calculator v2

Calculates a composite score (0-100) for wallet quality based on:
- Performance (ROI)
- Consistency (win rate, trade frequency)
- Risk management (drawdown)
- Statistical significance (trade count)

WQS v2 improvements:
- Anti-pump-and-dump: Penalizes recent massive spikes
- Statistical significance: Low confidence penalty for < 20 trades
- Drawdown penalty: Heavy penalty for high drawdowns
"""

from dataclasses import dataclass
from typing import Optional


@dataclass
class WalletMetrics:
    """Wallet performance metrics for WQS calculation."""
    address: str
    roi_7d: Optional[float] = None
    roi_30d: Optional[float] = None
    trade_count_30d: Optional[int] = None
    win_rate: Optional[float] = None  # 0.0 to 1.0
    max_drawdown_30d: Optional[float] = None  # percentage
    avg_trade_size_sol: Optional[float] = None
    last_trade_at: Optional[str] = None
    win_streak_consistency: Optional[float] = None  # 0.0 to 1.0


def calculate_wqs(metrics: WalletMetrics) -> float:
    """
    Calculate Wallet Quality Score (WQS) v2.
    
    Scoring breakdown:
    - ROI performance: up to 40 points
    - Win streak consistency: up to 30 points
    - Activity bonus: up to 10 points
    - Anti-pump-and-dump: -15 points if 7d ROI > 2x 30d ROI
    - Statistical significance: 0.5x multiplier if < 20 trades
    - Drawdown penalty: -0.2 * drawdown_percent
    
    Args:
        metrics: WalletMetrics object with wallet data
        
    Returns:
        WQS score from 0 to 100
    """
    # PDD specification: score starts at 0.
    score = 0.0

    # 1) ROI Performance (up to 40 points)
    #
    # We intentionally allow strong wallets to exceed 70 overall so ACTIVE is reachable.
    # Piecewise mapping:
    # - 0%..100% ROI -> 0..35 points
    # - 100%..200% ROI -> 35..40 points
    if metrics.roi_30d is not None and metrics.roi_30d > 0:
        roi = metrics.roi_30d
        if roi <= 100.0:
            score += (roi / 100.0) * 35.0
        else:
            roi_over = min(roi, 200.0) - 100.0
            score += 35.0 + (roi_over / 100.0) * 5.0

    # 2) Consistency (up to 30 points)
    if metrics.win_streak_consistency is not None:
        score += max(0.0, min(metrics.win_streak_consistency, 1.0)) * 30.0
    elif metrics.win_rate is not None:
        # Fallback: use win rate as proxy for consistency.
        score += max(0.0, min(metrics.win_rate, 1.0)) * 25.0

    # 3) Activity bonus (up to 10 points)
    # Smooth ramp: 20 trades -> 0 points, 50+ trades -> full 10 points.
    if metrics.trade_count_30d is not None:
        tc = max(0, metrics.trade_count_30d)
        if tc >= 50:
            score += 10.0
        elif tc > 20:
            score += ((tc - 20) / 30.0) * 10.0

    # 4) Anti-Pump-and-Dump Check
    # Penalize wallets with recent massive spikes (likely lucky trades)
    if metrics.roi_7d is not None and metrics.roi_30d is not None:
        if metrics.roi_30d > 0 and metrics.roi_7d > metrics.roi_30d * 2:
            score -= 15.0
    
    # 5) Statistical Significance
    # Low confidence penalty for wallets with few trades
    # Check <10 first (stronger penalty), then <20 (weaker penalty)
    if metrics.trade_count_30d is not None:
        if metrics.trade_count_30d < 10:
            score *= 0.25  # Strong penalty for very few trades
        elif metrics.trade_count_30d < 20:
            score *= 0.5   # Moderate penalty for low trade count
    
    # 6) Drawdown Penalty
    # High drawdown indicates poor risk management
    if metrics.max_drawdown_30d is not None:
        score -= metrics.max_drawdown_30d * 0.2
    
    # Clamp to 0-100 range
    return max(0.0, min(score, 100.0))


def classify_wallet(wqs_score: float) -> str:
    """
    Classify wallet based on WQS score.
    
    Returns:
        'ACTIVE', 'CANDIDATE', or 'REJECTED'
    """
    if wqs_score >= 70.0:
        return "ACTIVE"
    elif wqs_score >= 40.0:
        return "CANDIDATE"
    else:
        return "REJECTED"


# Example usage
if __name__ == "__main__":
    # Test with sample data
    test_metrics = WalletMetrics(
        address="7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        roi_7d=15.0,
        roi_30d=45.0,
        trade_count_30d=127,
        win_rate=0.72,
        max_drawdown_30d=8.5,
        win_streak_consistency=0.65,
    )
    
    wqs = calculate_wqs(test_metrics)
    status = classify_wallet(wqs)
    
    print(f"Address: {test_metrics.address[:8]}...")
    print(f"WQS Score: {wqs:.1f}")
    print(f"Classification: {status}")
