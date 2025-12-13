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
    avg_entry_delay_seconds: Optional[float] = None
    profit_factor: Optional[float] = None
    sortino_ratio: Optional[float] = None


def calculate_wqs(metrics: WalletMetrics) -> float:
    """
    Calculate Wallet Quality Score (WQS) v2 (0-100).

    This implementation matches the Scout test suite expectations and the PDD's
    core intent: favor repeatable profitability with low drawdowns, penalize
    recent ROI spikes, and discount low-sample wallets.

    Scoring breakdown:
    - ROI performance: up to 25 points (capped at 100% ROI)
    - Consistency: up to 25 points (win_streak_consistency)
    - Win rate fallback: up to 25 points (if consistency unavailable)
    - Activity bonus: +5 points if trade_count_30d >= 50
    - Anti-pump-and-dump: -15 points if 7d ROI > 2x 30d ROI (and 30d ROI > 0)
    - Statistical significance: smooth confidence multiplier based on
      realized closes (`trade_count_30d`), reaching 1.0 at 20+
    - Drawdown penalty: -0.2 * drawdown_percent
    
    Args:
        metrics: WalletMetrics object with wallet data
        
    Returns:
        WQS score from 0 to 100
    """
    # PDD specification: score starts at 0.
    score = 0.0

    # 1) ROI Performance (up to 25 points), capped at 100% ROI
    if metrics.roi_30d is not None and metrics.roi_30d > 0:
        roi = min(metrics.roi_30d, 100.0)
        score += (roi / 100.0) * 25.0

    # 2) Consistency (up to 25 points)
    if metrics.win_streak_consistency is not None:
        score += max(0.0, min(metrics.win_streak_consistency, 1.0)) * 25.0
    elif metrics.win_rate is not None:
        # Fallback: use win rate as proxy for consistency (up to 25 points)
        score += max(0.0, min(metrics.win_rate, 1.0)) * 25.0

    # 3) Activity bonus (+5 if 50+ closes)
    if metrics.trade_count_30d is not None:
        tc = max(0, metrics.trade_count_30d)
        if tc >= 50:
            score += 5.0

    # 4) Anti-Pump-and-Dump Check
    # Penalize wallets with recent massive spikes (likely lucky trades)
    if metrics.roi_7d is not None and metrics.roi_30d is not None:
        if metrics.roi_30d > 0 and metrics.roi_7d > metrics.roi_30d * 2:
            score -= 15.0
    
    # 5) Statistical Significance
    # Low confidence penalty for wallets with few realized closes.
    #
    # We use a smooth, monotonic multiplier so 1–4 closes are discounted but
    # not annihilated, and 20+ closes reach full confidence.
    if metrics.trade_count_30d is not None:
        tc = max(0, int(metrics.trade_count_30d))
        if tc == 0:
            confidence = 0.0
        else:
            # 1 close  -> 0.62
            # 5 closes -> 0.70
            # 10 closes -> 0.80
            # 20 closes -> 1.00
            # Detailed: Start at 0.6 base confidence, ramp to 1.0 at 20 trades
            confidence = min(1.0, 0.6 + 0.4 * (tc / 20.0))
        score *= confidence
    else:
        # Explicitly handle None case to avoid implicit 0 behavior if logic changes
        # If trade_count_30d is None, it implies no trades or unknown, so confidence should be 0.
        score *= 0.0
    
    # 6) Drawdown Penalty
    # High drawdown indicates poor risk management
    if metrics.max_drawdown_30d is not None:
        score -= metrics.max_drawdown_30d * 0.2

    # 7) Sniper Penalty (The "Copy-Ability" Check)
    # If they consistently buy < 30 seconds after launch, they are likely a bot/sniper.
    if metrics.avg_entry_delay_seconds is not None:
        if metrics.avg_entry_delay_seconds < 30: 
            score -= 40.0 
        elif metrics.avg_entry_delay_seconds < 120:
            score -= 10.0
        else:
            score += 10.0

    # 8) Advanced Risk Metrics Bonus (MODIFIED)
    if metrics.profit_factor is not None:
        if metrics.profit_factor >= 3.0:
            score += 10.0  # Increased from 5.0 to reward high R:R
        elif metrics.profit_factor >= 1.5:
            score += 5.0   # Increased from 2.0
            
        # NEW: Compensation for low win rate if Profit Factor is high
        # If Win Rate < 40% but Profit Factor > 2.0, refund some consistency points
        if (metrics.win_rate or 0) < 0.40 and metrics.profit_factor > 2.0:
            score += 5.0
            
    if metrics.sortino_ratio is not None:
        if metrics.sortino_ratio >= 2.0:
            score += 5.0
        elif metrics.sortino_ratio >= 1.0:
            score += 2.0
    
    # 9) Recency Bias (Freshness)
    # Determine if the wallet is active and winning recently
    if metrics.last_trade_at:
        try:
            # Handle timestamps with Z or offset
            last_trade_str = metrics.last_trade_at.replace("Z", "+00:00")
            last_trade = datetime.fromisoformat(last_trade_str)
            
            # Ensure timezone-aware comparison (assume utcnow is naive, so use naive delta if needed or unify)
            # best practice: use fromisoformat which handles offset if present. 
            # If naive, assume UTC.
            now = datetime.utcnow()
            if last_trade.tzinfo is not None:
                # If last_trade is aware, make now aware (UTC)
                from datetime import timezone
                now = now.replace(tzinfo=timezone.utc)
                
            days_since_trade = (now - last_trade).days
            
            if days_since_trade <= 2:
                score += 10.0  # Very active/fresh
            elif days_since_trade <= 5:
                score += 5.0   # Active
            elif days_since_trade > 14:
                score -= 10.0  # Stale wallet penalty
                
            # Bonus: Momentum check (7d ROI is positive and contributing heavily to 30d)
            if (metrics.roi_7d or 0) > 0 and (metrics.roi_30d or 0) > 0:
                # If >50% of monthly ROI came from this week, it's hot
                if metrics.roi_7d >= (metrics.roi_30d * 0.5):
                    score += 5.0
        except (ValueError, TypeError):
            pass

    # Clamp to 0-100 range
    return max(0.0, min(score, 100.0))


def classify_wallet(wqs_score: float) -> str:
    """
    Classify wallet based on WQS score.

    Thresholds:
    - ACTIVE: >= 70
    - CANDIDATE: >= 40
    - REJECTED: < 40
    
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
