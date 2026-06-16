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
from datetime import datetime, timezone
import os


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
    is_unproven: bool = False  # No closed-trade PnL: mid-position or unknown track record
    parse_rate: Optional[float] = None  # Fraction of transactions that parsed successfully (0.0-1.0)
    total_unrealized_loss_sol: Optional[float] = None  # Unrealized PnL from bag holdings
    total_realized_profit_sol: Optional[float] = None  # Total realized profit for comparison
    dex_diversity_score: Optional[int] = None  # Count of unique DEXs used
    uses_limit_orders: bool = False  # Detected Jupiter Limit Order usage
    uses_mev_protection: bool = False  # Detected Jito bundle/MEV protection usage
    correlated_with_scam: bool = False  # Wallet or funder on known scam denylist


@dataclass
class RawScoreComponents:
    """Separated bonus and penalty contributions for confidence-aware scoring."""
    positive: float = 0.0
    negative: float = 0.0   # stored as absolute value (positive number)
    is_instant_reject: bool = False

    @property
    def raw_score(self) -> float:
        """Traditional combined score (positive - negative), clamped to [0, 100]."""
        return max(0.0, min(self.positive - self.negative, 100.0))


def _calculate_raw_score(metrics: WalletMetrics) -> RawScoreComponents:
    """
    Compute bonus and penalty components of the raw quality score (0-100).

    Separates positive contributions (bonuses) from negative ones (penalties)
    so that the confidence multiplier can be applied to bonuses only,
    while penalties retain full weight regardless of sample size.

    Returns RawScoreComponents with is_instant_reject=True for categorically
    uncopyable wallets (snipers) that bypass the normal scoring pipeline.
    """
    positive = 0.0
    negative = 0.0

    # 1) ROI Base Score (0-35 pts)
    # Reward consistent positive ROI over 7d and 30d.
    # When recency weighting is enabled, 7d performance carries more weight.
    roi_7d = metrics.roi_7d or 0.0
    roi_30d = metrics.roi_30d or 0.0

    # Check if recency-weighted scoring is enabled
    try:
        from config import ScoutConfig
        _use_recency = ScoutConfig.get_wqs_recency_weight() if ScoutConfig else True
    except ImportError:
        _use_recency = os.environ.get("SCOUT_WQS_RECENCY_WEIGHT", "true").lower() == "true"

    # Check for pump-and-dump pattern BEFORE applying recency weighting,
    # so that pump wallets don't benefit from recent-performance boosts.
    # 
    # For wallets with positive 30d ROI: flag if 7d > 2x 30d (classic pump).
    # For wallets with negative 30d ROI: only flag if 7d is extreme relative to
    # the loss magnitude AND the absolute 7d return is >50%. This avoids false-flagging
    # genuine recoveries (e.g., -10% 30d → +20% 7d) as pump spikes.
    if roi_30d > 0:
        _is_pump_spike = roi_7d > max(roi_30d * 2.0, 5.0)
    else:
        _is_pump_spike = roi_7d > max(abs(roi_30d) * 3.0, 15.0) and roi_7d > 50

    if _use_recency and not _is_pump_spike and roi_30d > 0 and roi_7d > 0:
        # Standard 30d ROI contribution
        base_30d = min(25.0, (roi_30d / 100.0) * 25.0)
        # Time-weighted ROI: blends recent (7d) and full-month (30d)
        weighted_roi = roi_7d * 0.5 + roi_30d * 0.5
        recency_score = min(25.0, (weighted_roi / 100.0) * 25.0)
        # Use the better of standard and recency-weighted, so recency only helps
        positive += max(base_30d, recency_score)
        # Bonus for wallets where recent > monthly (upward momentum confirmed)
        if roi_7d > roi_30d * 0.6:
            positive += 5.0
    else:
        if roi_30d > 0:
            positive += min(25.0, (roi_30d / 100.0) * 25.0)

    if roi_7d > 0:
        positive += min(10.0, (roi_7d / 100.0) * 10.0)

    # Consistency Bonus: 7d is positive defined as > -5% (allow small pullback)
    # and 30d is solid.
    if roi_7d > -5.0 and roi_30d > 20.0:
        positive += 10.0

    # 2) Win Rate & Profit Factor (0-20 pts)
    # Win-rate bonuses above +10 require sound profit factor to prevent
    # Martingale wallets (high win rate, catastrophic losses) from passing.
    win_rate = metrics.win_rate or 0.0
    profit_factor = metrics.profit_factor

    if win_rate >= 0.5:
        positive += 5.0
    if win_rate >= 0.65:
        positive += 5.0
    # Tiers above +10 require profit_factor >= 1.2 to gate Martingale risk
    if win_rate >= 0.80 and (profit_factor is None or profit_factor >= 1.2):
        positive += 5.0
    if win_rate >= 0.90 and (profit_factor is None or profit_factor >= 1.2):
        positive += 5.0
        
            
    # 3) Activity Level (0-20 pts)
    count = metrics.trade_count_30d or 0
    
    # Monotonic increase up to saturation
    if count >= 5:
        positive += 2.0
    if count >= 10:
        positive += 3.0
    if count >= 20:
        positive += 5.0
    if count >= 50:
        positive += 5.0
    if count >= 100:
        positive += 5.0  # Grinder bonus
    
    # 4) Penalties (Drawdown & Pump-Dump)
    dd = metrics.max_drawdown_30d or 0.0

    # Linear drawdown penalty: -0.2 points per percent of drawdown
    negative += dd * 0.2

    # Anti-Pump-and-Dump / Lucky Shot Check
    # If 7d ROI is unusually large relative to the 30d baseline, it's likely a lucky
    # pump we cannot reliably replicate. Use abs(roi_30d) so the spike threshold
    # scales correctly even when the monthly trend is negative — a wallet with
    # -10% monthly but +50% weekly is a lucky spike, not a recovery trend.
    if _is_pump_spike:
        negative += 25.0  # Anti pump-and-dump (increased from 17 to offset recency boost)
        
    # 5) Scalability / Liquidity Safety (Implicit in avg_trade_size)
    if (metrics.avg_trade_size_sol or 0) < 0.05:
        negative += 10.0  # Dust trader, hard to copy profitably due to fixed gas

    # 6) Consistency (Win Streak)
    if metrics.win_streak_consistency and metrics.win_streak_consistency > 0.4:
        positive += 5.0

    # 7) Sniper / Bot Penalty (Critical for Copy Trading)
    if metrics.avg_entry_delay_seconds is not None:
        # If they buy < 30s after launch on average, they are likely a bot/sniper.
        # We cannot copy them profitably due to MEV/Latency.
        if metrics.avg_entry_delay_seconds < 30:
            return RawScoreComponents(is_instant_reject=True)
        
        # If they buy < 60s, moderately penalize
        elif metrics.avg_entry_delay_seconds < 60:
            negative += 15.0
            
        # If they wait 2 mins - 1 hour, they are "Smart Money" (Human/Algo analysis)
        # This is the "Sweet Spot" for copy trading.
        elif 120 < metrics.avg_entry_delay_seconds < 3600:
            positive += 15.0

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
            positive += 15.0  # Elite trader
        elif metrics.profit_factor > 1.5:
            positive += 5.0   # Profitable
        elif metrics.profit_factor >= 1.2:
            positive += 2.0   # Marginally profitable
        elif metrics.profit_factor < 1.0:
            # Losing trader (Gross Loss > Gross Win)
            # Graduated: harsher penalty for deeply negative PF, lighter for near-breakeven
            if metrics.profit_factor < 0.5:
                negative += 40.0
            else:
                negative += 25.0
        elif metrics.profit_factor < 1.2:
            # Martingale Zone: Profitable but barely. High risk of blowup.
            negative += 10.0
    
    # Unproven wallet penalty (continuous based on parse rate when available)
    # When < 30% of transactions parse successfully, the wallet's trading activity
    # is too opaque to evaluate reliably. The penalty scales with parse quality
    # so a wallet with 28% parse rate gets a lighter penalty than one with 1%.
    # Falls back to the binary -20 penalty when parse_rate is not available.
    if metrics.parse_rate is not None:
        if metrics.parse_rate < 0.60:
            continuous_penalty = (0.60 - metrics.parse_rate) * 80.0  # 0% parse → -48, 58% parse → -1.6
            negative += continuous_penalty
    elif metrics.is_unproven:
        negative += 20.0

    # Explicit Martingale Pattern Detection
    # A wallet with win_rate > 0.7 AND profit_factor < 1.5 is playing a classic
    # Martingale strategy: wins often but loses heavily, heading for eventual blow-up.
    # This penalty is additional to any profit_factor penalty already applied above.
    if profit_factor is not None and win_rate > 0.70 and profit_factor < 1.5:
        negative += 15.0

    # 8) Sortino/Sharpe Proxy
    # Sortino ratio measures risk-adjusted return (downside deviation only).
    # Higher weight than before: this is a powerful signal that was underutilized.
    if metrics.sortino_ratio is not None:
        if metrics.sortino_ratio >= 3.0:
            positive += 12.0
        elif metrics.sortino_ratio >= 2.0:
            positive += 8.0
        elif metrics.sortino_ratio >= 1.0:
            positive += 4.0
        elif metrics.sortino_ratio >= 0.5:
            positive += 2.0
        elif metrics.sortino_ratio < 0:
            negative += 10.0  # Downside volatility exceeds return
    
    # 9) Insider / Fresh Wallet Penalty
    # Fresh wallets (created <24h before trading) are typically burners/insiders.
    # We penalize them heavily to avoid copying ephemeral addresses.
    if metrics.is_fresh_wallet:
        negative += 10.0

    # D5: Scam correlation penalty — downgrade wallets linked to known rug/scam clusters.
    # Even a wallet with strong metrics is risky if it's in the same ring as scammers.
    if metrics.correlated_with_scam:
        negative += 20.0
    
    # 10) Smart Money Bonuses
    # DEX Diversity: Using multiple DEXs shows sophistication
    if metrics.dex_diversity_score is not None and metrics.dex_diversity_score >= 3:
        positive += 5.0

    # Limit Orders: Sophisticated trading strategy
    if metrics.uses_limit_orders:
        positive += 10.0

    # MEV Protection: Shows awareness of MEV risks
    if metrics.uses_mev_protection:
        positive += 10.0

    # Cap smart money bonuses to prevent inflating scores without profit proof.
    # A wallet with mediocre trading performance can accumulate up to 25 points from
    # tool usage alone (5 DEX + 10 limit + 10 MEV), potentially pushing a sub-60
    # raw score above the ACTIVE threshold despite poor copy-trading viability.
    # Only award smart money bonuses if the wallet has demonstrated positive ROI,
    # ensuring tools are rewarded alongside results rather than as a substitute.
    should_remove_bonuses = False
    if metrics.roi_30d is not None and metrics.roi_30d < -10:
        should_remove_bonuses = True
    if metrics.profit_factor is not None and metrics.profit_factor < 1.2:
        should_remove_bonuses = True
    if metrics.win_rate is not None and metrics.win_rate < 0.45:
        should_remove_bonuses = True

    if should_remove_bonuses:
        # Remove bonuses added above by reducing the positive accumulator
        if metrics.dex_diversity_score is not None and metrics.dex_diversity_score >= 3:
            positive -= 5.0
        if metrics.uses_limit_orders:
            positive -= 10.0
        if metrics.uses_mev_protection:
            positive -= 10.0
    
    # 11) Bag Holder Penalty (The "Hidden Loser" Detector)
    # If unrealized losses > 50% of realized gains, this is a bad trader
    # They sell winners and hold losers, making them look profitable when they're not.
    if metrics.total_unrealized_loss_sol is not None and metrics.total_realized_profit_sol is not None:
        if metrics.total_realized_profit_sol > 0:
            loss_ratio = metrics.total_unrealized_loss_sol / metrics.total_realized_profit_sol
            if loss_ratio > 0.5:  # Losses > 50% of gains — severe bag holder
                negative += 30.0
            elif loss_ratio > 0.2:  # Losses > 20% of gains — moderate concern
                negative += 10.0
        elif metrics.total_unrealized_loss_sol > 0:  # Has unrealized losses but no realized profit
            negative += 20.0
    
    # 11) Recency Bias (Freshness)
    # Determine if the wallet is active and winning recently
    if metrics.last_trade_at:
        try:
            # Handle timestamps with Z or offset
            last_trade_str = metrics.last_trade_at.replace("Z", "+00:00")
            last_trade = datetime.fromisoformat(last_trade_str)
            
            # Ensure timezone-aware comparison
            # best practice: use fromisoformat which handles offset if present.
            # If naive, assume UTC.
            now = datetime.now(timezone.utc)
            if last_trade.tzinfo is None:
                # If last_trade is naive, convert now to a naive UTC datetime
                now = now.replace(tzinfo=None)
                
            days_since_trade = (now - last_trade).days
            
            if days_since_trade <= 2:
                positive += 10.0  # Very active/fresh
            elif days_since_trade <= 5:
                positive += 5.0   # Active
            elif days_since_trade > 14:
                negative += 10.0  # Stale wallet penalty
                
            # Momentum check: reward hot wallets, penalize cooling wallets.
            # A wallet that earned 80% of its monthly ROI last week is actively good;
            # one that peaked 3 weeks ago (7d << 30d) is drifting and warrants a penalty.
            roi_7d = metrics.roi_7d or 0
            roi_30d = metrics.roi_30d or 0
            if roi_7d > 0 and roi_30d > 0:
                if roi_7d >= (roi_30d * 0.5):
                    positive += 5.0  # Hot wallet: recent outperformance
            if roi_30d > 0 and roi_7d < (roi_30d * 0.3):
                negative += 10.0  # Cooling wallet: peaked weeks ago, now underperforming
        except (ValueError, TypeError):
            pass

    return RawScoreComponents(positive=positive, negative=negative)


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

    Confidence is applied ONLY to positive score components (bonuses).
    Penalties retain full weight regardless of sample size, so wallets
    with suspicious patterns (pump-and-dump, high drawdown, bag-holding)
    are penalized MORE at low trade counts, not less.

    Args:
        metrics: WalletMetrics object with wallet data

    Returns:
        WQS score from 0 to 100
    """
    components = _calculate_raw_score(metrics)
    if components.is_instant_reject:
        return 0.0

    trade_count = metrics.trade_count_30d or 0
    confidence = _compute_confidence(trade_count, metrics.profit_factor, metrics, metrics.is_unproven)
    adjusted = max(0.0, components.positive * confidence - components.negative)
    return max(0.0, min(adjusted, 100.0))


def _compute_confidence(trade_count: int, profit_factor: Optional[float] = None, metrics: Optional[WalletMetrics] = None, is_unproven: bool = False) -> float:
    """
    Statistical confidence based on trade count and trade size.

    Uses a three-region curve:
    - Below 3 trades: linear 0→0.55 (very sparse data)
    - 3-10 trades:     linear 0.55→0.90 (emerging pattern)
    - 10-20 trades:    linear 0.90→1.0 (meaningful sample)
    - 20+ trades:      1.0 (full confidence)

    Includes a profit-factor override: if profit_factor > 2.0 AND trade_count >= 3,
    confidence is raised to at least 0.80, since a wallet with high-quality trades
    deserves more weight even with a small (but not trivial) sample. Single-trade
    wallets are excluded because one winning trade gives infinite PF.

    Trade size weighting (Phase 2.2): avg_trade_size_sol is factored into confidence
    so that wallets with economically meaningful trades get higher confidence.
    A whale with 15 trades of 100 SOL gets more confidence than a dust trader
    with 100 trades of 0.0001 SOL.

    For unproven wallets (< 30% parse rate), confidence is capped at 0.70
    regardless of trade count, since opaque trading activity cannot be
    evaluated reliably.
    """
    confidence = 0.0
    if trade_count >= 20:
        confidence = 1.0
    elif trade_count >= 10:
        confidence = 0.90 + 0.10 * (trade_count - 10) / 10.0
    elif trade_count >= 3:
        confidence = 0.55 + 0.35 * (trade_count - 3) / 7.0
    else:
        confidence = (trade_count / 3.0) * 0.55

    if profit_factor is not None and profit_factor > 2.0 and trade_count >= 3 and confidence < 0.80:
        confidence = 0.80

    # Size-weighted confidence: blend trade count confidence with trade size factor.
    # A wallet with 100 dust trades gets lower real-world confidence than a whale
    # with 15 meaningful trades, even though the raw trade count would suggest otherwise.
    if metrics is not None and metrics.avg_trade_size_sol is not None:
        avg_size = metrics.avg_trade_size_sol
        size_factor = min(1.0, avg_size / 0.5)  # Full size confidence at 0.5 SOL avg
        confidence = confidence * (0.5 + 0.5 * size_factor)  # 50% weight on size

    if is_unproven and confidence > 0.70:
        confidence = 0.70

    # Continuous confidence cap based on parse_rate (Phase 2.3)
    # Lower parse rates cap confidence more aggressively.
    if metrics is not None and metrics.parse_rate is not None:
        parse_cap = 0.30 + metrics.parse_rate * 0.70  # 0% parse → 0.30 cap, 100% parse → 1.0 cap
        if confidence > parse_cap:
            confidence = parse_cap

    return max(0.0, min(confidence, 1.0))


def calculate_wqs_with_confidence(metrics: WalletMetrics) -> WqsResult:
    """
    Like calculate_wqs() but returns quality score and sample confidence separately.

    Use when the caller needs to distinguish between a wallet that is *bad* vs one
    that is *unproven* — they both produce a low adjusted_score but for different
    reasons, and the Operator's position sizer handles them differently.
    """
    components = _calculate_raw_score(metrics)
    if components.is_instant_reject:
        return WqsResult(score=0.0, confidence=0.0, adjusted_score=0.0)

    trade_count = metrics.trade_count_30d or 0
    confidence = _compute_confidence(trade_count, metrics.profit_factor, metrics, metrics.is_unproven)
    adjusted_score = max(0.0, min(components.positive * confidence - components.negative, 100.0))
    return WqsResult(score=components.raw_score, confidence=confidence, adjusted_score=adjusted_score)


def classify_wallet(
    wqs_score: float,
    active_threshold: float = 65.0,
    candidate_threshold: float = 20.0,
) -> str:
    """
    Classify wallet based on WQS score.

    Args:
        wqs_score: Computed WQS (0-100)
        active_threshold: Min score for ACTIVE status (default 65.0)
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
