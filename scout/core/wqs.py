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
    - ROI performance: up to 25 points
    - Win streak consistency: up to 20 points
    - Anti-pump-and-dump: -15 points if 7d ROI > 2x 30d ROI
    - Statistical significance: 0.5x multiplier if < 20 trades
    - Drawdown penalty: -0.2 * drawdown_percent
    
    Args:
        metrics: WalletMetrics object with wallet data
        
    Returns:
        WQS score from 0 to 100
    """
    score = 0.0
    
    # Base score starts at 50 (neutral)
    score = 50.0
    
    # 1. ROI Performance (up to 25 points)
    if metrics.roi_30d is not None:
        # Cap ROI contribution at 100% for score calculation
        roi_capped = min(metrics.roi_30d, 100.0)
        score += (roi_capped / 100.0) * 25.0
    
    # 2. Win Streak Consistency (up to 20 points)
    if metrics.win_streak_consistency is not None:
        score += metrics.win_streak_consistency * 20.0
    elif metrics.win_rate is not None:
        # Fallback: use win rate as proxy for consistency
        score += metrics.win_rate * 15.0
    
    # 3. Anti-Pump-and-Dump Check
    # Penalize wallets with recent massive spikes (likely lucky trades)
    if metrics.roi_7d is not None and metrics.roi_30d is not None:
        if metrics.roi_30d > 0 and metrics.roi_7d > metrics.roi_30d * 2:
            score -= 15.0
    
    # 4. Statistical Significance
    # Low confidence penalty for wallets with few trades
    if metrics.trade_count_30d is not None:
        if metrics.trade_count_30d < 20:
            score *= 0.5
        elif metrics.trade_count_30d < 10:
            score *= 0.25
    
    # 5. Drawdown Penalty
    # High drawdown indicates poor risk management
    if metrics.max_drawdown_30d is not None:
        score -= metrics.max_drawdown_30d * 0.2
    
    # 6. Activity Bonus (small bonus for active traders)
    if metrics.trade_count_30d is not None and metrics.trade_count_30d >= 50:
        score += 5.0
    
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
