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
from datetime import datetime


@dataclass
class WqsResult:
    """Result of a WQS calculation with quality score and sample confidence separated."""
    score: float          # Raw quality score before confidence weighting (0-100)
    confidence: float     # Sample confidence 0.0-1.0 (reaches 1.0 at 20+ trades)
    adjusted_score: float # score * confidence, clamped 0-100 — use for routing decisions


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
    is_fresh_wallet: bool = False  # Insider/Burner detection
    total_unrealized_loss_sol: Optional[float] = None  # Unrealized PnL from bag holdings
    total_realized_profit_sol: Optional[float] = None  # Total realized profit for comparison
    dex_diversity_score: Optional[int] = None  # Count of unique DEXs used
    uses_limit_orders: bool = False  # Detected Jupiter Limit Order usage
    uses_mev_protection: bool = False  # Detected Jito bundle/MEV protection usage


def _calculate_raw_score(metrics: WalletMetrics) -> float:
    """
    Compute the raw quality score (0-100) *without* the confidence multiplier.

    Separating quality from sample-size confidence lets callers decide how to
    weight the two independently (e.g. `calculate_wqs_with_confidence`).
    Returns 0.0 immediately for wallets that are categorically uncopyable (snipers).
    """
    # PDD specification: score starts at 0.
    score = 0.0

    # 1) ROI Base Score (0-35 pts)
    # Reward consistent positive ROI over 7d and 30d
    roi_7d = metrics.roi_7d or 0.0
    roi_30d = metrics.roi_30d or 0.0

    if roi_30d > 0:
        score += min(25.0, (roi_30d / 100.0) * 25.0)  # Cap at 100% ROI = 25 pts

    if roi_7d > 0:
        score += min(10.0, (roi_7d / 100.0) * 10.0)  # Cap at 100% ROI = 10 pts (matches 30d scaling)

    # Consistency Bonus: 7d is positive defined as > -5% (allow small pullback)
    # and 30d is solid.
    if roi_7d > -5.0 and roi_30d > 20.0:
        score += 10.0

    # 2) Win Rate & Profit Factor (0-20 pts)
    win_rate = metrics.win_rate or 0.0
    
    if win_rate >= 0.5:
        score += 5.0
    if win_rate >= 0.65:
        score += 5.0
        
            
    # 3) Activity Level (0-20 pts)
    count = metrics.trade_count_30d or 0
    
    # Monotonic increase up to saturation
    if count >= 5:
        score += 2.0
    if count >= 10:
        score += 3.0
    if count >= 20:
        score += 5.0
    if count >= 50:
        score += 5.0
    if count >= 100:
        score += 5.0  # Grinder bonus
    
    # 4) Penalties (Drawdown & Pump-Dump)
    dd = metrics.max_drawdown_30d or 0.0

    # Linear drawdown penalty: -0.2 points per percent of drawdown
    score -= dd * 0.2

    # Anti-Pump-and-Dump / Lucky Shot Check
    # If 7d ROI is unusually large relative to the 30d baseline, it's likely a lucky
    # pump we cannot reliably replicate. Use abs(roi_30d) so the spike threshold
    # scales correctly even when the monthly trend is negative — a wallet with
    # -10% monthly but +50% weekly is a lucky spike, not a recovery trend.
    if roi_7d > max(abs(roi_30d) * 2.0, 5.0):
        score -= 17.0  # Anti pump-and-dump
        
    # 5) Scalability / Liquidity Safety (Implicit in avg_trade_size)
    if (metrics.avg_trade_size_sol or 0) < 0.05:
        score -= 10.0  # Dust trader, hard to copy profitably due to fixed gas

    # 6) Consistency (Win Streak)
    if metrics.win_streak_consistency and metrics.win_streak_consistency > 0.4:
        score += 5.0

    # 7) Sniper / Bot Penalty (Critical for Copy Trading)
    if metrics.avg_entry_delay_seconds is not None:
        # If they buy < 30s after launch on average, they are likely a bot/sniper.
        # We cannot copy them profitably due to MEV/Latency.
        if metrics.avg_entry_delay_seconds < 30:
            return 0.0 # IMMEDIATE REJECTION - DO NOT PASS 
        
        # If they buy < 60s, heavily penalize
        elif metrics.avg_entry_delay_seconds < 60:
            score -= 30.0
            
        # If they wait 2 mins - 1 hour, they are "Smart Money" (Human/Algo analysis)
        # This is the "Sweet Spot" for copy trading.
        elif 120 < metrics.avg_entry_delay_seconds < 3600:
            score += 15.0

    # ---------------------------------------------------------
    # IMPROVED: Profit Factor Logic (Martingale Risk Detection)
    # ---------------------------------------------------------
    # Win rate is easily faked (sell winners, hold losers). 
    # Profit Factor (Total Gains / Total Losses) exposes bag holders.
    # 
    # CRITICAL: High win rate but low profit factor = Martingale risk
    # Example: Wins 90% of trades taking $1 profit, loses 10% taking $100 loss
    # This trader will eventually blow up when they hit a losing streak.
    if metrics.profit_factor is not None:
        if metrics.profit_factor > 3.0:
            score += 15.0  # Elite trader
        elif metrics.profit_factor > 1.5:
            score += 5.0   # Profitable
        elif metrics.profit_factor < 1.0:
            # Losing trader (Gross Loss > Gross Win)
            score -= 40.0
        elif metrics.profit_factor < 1.2:
            # Martingale Zone: Profitable but barely. High risk of blowup.
            score -= 20.0
    else:
        # No closed-trade PnL data — unproven, penalise mildly to avoid ranking
        # above wallets that have a demonstrated (if modest) profit factor.
        score -= 5.0
    # 8) Sortino/Sharpe Proxy
    if metrics.sortino_ratio:
        if metrics.sortino_ratio >= 2.0:
            score += 5.0
        elif metrics.sortino_ratio >= 1.0:
            score += 2.0
    
    # 9) Insider / Fresh Wallet Penalty
    # Fresh wallets (created <24h before trading) are typically burners/insiders.
    # We penalize them heavily to avoid copying ephemeral addresses.
    if metrics.is_fresh_wallet:
        score -= 20.0
    
    # 10) Smart Money Bonuses
    # DEX Diversity: Using multiple DEXs shows sophistication
    if metrics.dex_diversity_score is not None and metrics.dex_diversity_score >= 3:
        score += 5.0
    
    # Limit Orders: Sophisticated trading strategy
    if metrics.uses_limit_orders:
        score += 10.0
    
    # MEV Protection: Shows awareness of MEV risks
    if metrics.uses_mev_protection:
        score += 10.0
    
    # 11) Bag Holder Penalty (The "Hidden Loser" Detector)
    # If unrealized losses > 50% of realized gains, this is a bad trader
    # They sell winners and hold losers, making them look profitable when they're not.
    if metrics.total_unrealized_loss_sol is not None and metrics.total_realized_profit_sol is not None:
        if metrics.total_realized_profit_sol > 0:
            loss_ratio = metrics.total_unrealized_loss_sol / metrics.total_realized_profit_sol
            if loss_ratio > 0.5:  # Losses > 50% of gains
                score -= 30.0  # Massive Penalty
            elif loss_ratio > 0.2:  # Losses > 20% of gains
                score -= 15.0
        elif metrics.total_unrealized_loss_sol > 0:  # Has any unrealized losses but no realized profit
            # Any unrealized loss with zero realized profit flags a bag holder
            score -= 20.0
    
    # 11) Recency Bias (Freshness)
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
                
            # Momentum check: reward hot wallets, penalize cooling wallets.
            # A wallet that earned 80% of its monthly ROI last week is actively good;
            # one that peaked 3 weeks ago (7d << 30d) is drifting and warrants a penalty.
            roi_7d = metrics.roi_7d or 0
            roi_30d = metrics.roi_30d or 0
            if roi_7d > 0 and roi_30d > 0:
                if roi_7d >= (roi_30d * 0.5):
                    score += 5.0  # Hot wallet: recent outperformance
            if roi_30d > 0 and roi_7d < (roi_30d * 0.3):
                score -= 10.0  # Cooling wallet: peaked weeks ago, now underperforming
        except (ValueError, TypeError):
            pass

    # Clamp to 0-100 range before returning the raw quality score.
    return max(0.0, min(score, 100.0))


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
    raw = _calculate_raw_score(metrics)
    trade_count = metrics.trade_count_30d or 0
    confidence = _compute_confidence(trade_count)
    return max(0.0, min(raw * confidence, 100.0))


def _compute_confidence(trade_count: int) -> float:
    """
    Statistical confidence based on trade count.

    Uses a two-region curve:
    - Below 5 trades: linear 0→0.70 (sparse data, aggressive penalty)
    - 5-20 trades:    linear 0.70→1.0 (meaningful sample, gentle scaling)
    - 20+ trades:     1.0 (full confidence)

    This prevents the prior formula (trade_count/20) from halving a 10-trade wallet's
    score to 50%, which pushed legitimate wallets below the ACTIVE threshold.
    """
    if trade_count >= 20:
        return 1.0
    if trade_count >= 5:
        # 5 trades → 0.70, 20 trades → 1.0
        return 0.70 + 0.30 * (trade_count - 5) / 15.0
    # 0 trades → 0.0, 5 trades → 0.70
    return (trade_count / 5.0) * 0.70


def calculate_wqs_with_confidence(metrics: WalletMetrics) -> WqsResult:
    """
    Like calculate_wqs() but returns quality score and sample confidence separately.

    Use when the caller needs to distinguish between a wallet that is *bad* vs one
    that is *unproven* — they both produce a low adjusted_score but for different
    reasons, and the Operator's position sizer handles them differently.
    """
    raw_score = _calculate_raw_score(metrics)
    trade_count = metrics.trade_count_30d or 0
    confidence = _compute_confidence(trade_count)
    adjusted_score = max(0.0, min(raw_score * confidence, 100.0))
    return WqsResult(score=raw_score, confidence=confidence, adjusted_score=adjusted_score)


def classify_wallet(
    wqs_score: float,
    active_threshold: float = 60.0,
    candidate_threshold: float = 20.0,
) -> str:
    """
    Classify wallet based on WQS score.

    Args:
        wqs_score: Computed WQS (0-100)
        active_threshold: Min score for ACTIVE status (default 60.0)
        candidate_threshold: Min score for CANDIDATE status (default 20.0)

    Returns:
        'ACTIVE', 'CANDIDATE', or 'REJECTED'
    """
    if wqs_score >= active_threshold:
        return "ACTIVE"
    elif wqs_score >= candidate_threshold:
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
