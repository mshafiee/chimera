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
from typing import Optional, Dict
from datetime import datetime, timezone
import os
import logging

logger = logging.getLogger(__name__)


class ScoreTracker:
    """Tracks per-component score contributions for adaptive weight calibration."""
    def __init__(self):
        self.positive = 0.0
        self.negative = 0.0
        self.components: Dict[str, float] = {}

    def add_pos(self, name: str, amount: float) -> None:
        self.positive += amount
        self.components[name] = self.components.get(name, 0.0) + amount

    def add_neg(self, name: str, amount: float) -> None:
        self.negative += amount
        self.components[name] = self.components.get(name, 0.0) - amount

    def to_components(self, is_instant_reject: bool = False) -> "RawScoreComponents":
        return RawScoreComponents(
            positive=self.positive,
            negative=self.negative,
            is_instant_reject=is_instant_reject,
            components=dict(self.components),
        )

    def _apply_penalty_precedence(self) -> None:
        """
        Apply penalty precedence: only keep the most severe penalty per category.

        This prevents multiple penalties from stacking excessively
        by keeping only the strongest penalty from each penalty category.
        """
        # Group penalties by category
        penalty_categories = {
            'martingale_penalty': [],  # Bag holder, paper gains, unproven
            'pump_spike_penalty': [],  # Pump and dump detection
            'sniper_penalty': [],  # Fast entry penalty
            'drawdown_penalty': [],  # Drawdown penalty
            'pf_wr_penalty': [],  # Profit factor to win rate ratio
            'mev_risk_penalty': [],  # MEV/sandwich risk
            'scam_penalty': [],  # Scam correlation
            'insider_penalty': [],  # Fresh wallet
            'smart_money_removal': [],  # Smart money removal
        }

        # Group penalties by category
        for name, value in self.components.items():
            if value < 0:  # It's a penalty
                # Determine category
                category = None
                if 'martingale' in name:
                    category = 'martingale_penalty'
                elif 'pump_spike' in name:
                    category = 'pump_spike_penalty'
                elif 'sniper' in name:
                    category = 'sniper_penalty'
                elif 'drawdown' in name:
                    category = 'drawdown_penalty'
                elif 'pf_wr' in name:
                    category = 'pf_wr_penalty'
                elif 'mev_risk' in name:
                    category = 'mev_risk_penalty'
                elif 'scam' in name:
                    category = 'scam_penalty'
                elif 'insider' in name:
                    category = 'insider_penalty'
                elif 'smart_money_removal' in name:
                    category = 'smart_money_removal'

                if category and category in penalty_categories:
                    penalty_categories[category].append((name, value))

        # For each category, only keep the most severe (largest absolute value) penalty
        for category, penalties in penalty_categories.items():
            if len(penalties) > 1:
                # Find the most severe penalty
                penalties.sort(key=lambda x: abs(x[1]), reverse=True)
                most_severe_name, most_severe_value = penalties[0]

                # Remove all other penalties in this category
                for other_name, _ in penalties[1:]:
                    if other_name in self.components:
                        del self.components[other_name]

                # Restore the most severe penalty
                self.components[most_severe_name] = most_severe_value

    def _apply_penalty_confidence(self) -> None:
        """
        Apply confidence weighting to uncertain penalties.

        Uncertain penalties (from sparse data) are reduced in impact
        while high-confidence penalties retain full weight.
        """
        # Penalty confidence mapping
        penalty_confidence = {
            'martingale_penalty': 0.8,  # High confidence (direct calculation)
            'pump_spike_penalty': 0.9,  # Very high confidence (clear signal)
            'sniper_penalty': 1.0,  # Certain (direct measurement)
            'drawdown_penalty': 0.9,  # High confidence (measured data)
            'pf_wr_penalty': 0.7,  # Medium confidence (ratio calculation)
            'mev_risk_penalty': 0.6,  # Medium-low confidence (heuristic)
            'scam_penalty': 1.0,  # Certain (denylist check)
            'insider_penalty': 0.8,  # High confidence (fresh wallet detection)
            'smart_money_removal': 0.5,  # Lower confidence (conditional)
        }

        for name, value in list(self.components.items()):
            if value < 0:  # It's a penalty
                # Determine penalty category
                confidence = 0.5  # Default medium confidence
                for category, conf in penalty_confidence.items():
                    if category in name:
                        confidence = conf
                        break

                # Apply confidence weighting
                # Reduce penalty impact if confidence is low
                if confidence < 0.8:
                    adjusted_value = value * (0.5 + 0.5 * confidence)
                    self.components[name] = adjusted_value

                    # Recalculate negative total
                    self.negative = abs(sum(v for v in self.components.values() if v < 0))


def _compute_wmi(roi_7d: Optional[float], roi_30d: Optional[float], trade_count_30d: Optional[int]) -> float:
    """
    Wallet Momentum Indicator — condensed from wmi.py core formula.
    Returns a score in [-1, 1]:
      +1.0 = strong positive momentum (accelerating)
       0.0 = stable
      -1.0 = strong negative momentum (actively degrading)
    """
    roi_trend = 0.0
    activity_trend = 0.0

    if roi_7d is not None and roi_30d is not None:
        if roi_30d > 0:
            roi_ratio = roi_7d / max(0.01, roi_30d)
            if roi_ratio > 0.5:
                roi_trend = min(1.0, (roi_ratio - 0.3) / 0.7)
            elif roi_ratio > 0.2:
                roi_trend = (roi_ratio - 0.2) / 0.3 * 0.5
            else:
                roi_trend = max(-1.0, roi_ratio - 0.7)
        elif roi_7d < 0:
            roi_trend = -0.5
        else:
            roi_trend = 0.0

    if trade_count_30d is not None and trade_count_30d > 0:
        activity_trend = max(-1.0, min(1.0, (trade_count_30d - 20) / 60.0))

    wqs_trend = roi_trend * 0.5 + activity_trend * 0.5
    wmi = wqs_trend * 0.4 + roi_trend * 0.3 + activity_trend * 0.3
    return max(-1.0, min(1.0, wmi))


def _interpret_trajectory(roi_7d: Optional[float], roi_30d: Optional[float]) -> str:
    """
    Interpret multi-timeframe trajectory from ROI data.
    Returns: "IMPROVING", "STABLE", "DECLINING", or "PEAKED"
    """
    if roi_7d is None or roi_30d is None:
        return "STABLE"
    if roi_30d > 10 and roi_7d < roi_30d * 0.2:
        return "PEAKED"
    if roi_30d > 0 and roi_7d < roi_30d * 0.3:
        return "DECLINING"
    if roi_7d > roi_30d * 0.6 and roi_30d > 5:
        return "IMPROVING"
    return "STABLE"


def _detect_smart_accumulation(metrics) -> float:
    """
    Detect smart accumulation patterns (gradual position building).

    Returns a score from 0.0 to 1.0 indicating how strongly the wallet
    shows accumulation behavior vs. panic buying.

    Patterns:
    - Accumulation: Gradual position building with increasing size
    - Pyramid up: Adding to winners (smart)
    - Average down: Adding to losers (risky)
    - FOMO: Large sudden positions (negative)
    """
    score = 0.0

    # Check trade size progression (need trade history)
    if not hasattr(metrics, 'trade_sizes') or not metrics.trade_sizes:
        return 0.0  # Can't detect without trade history

    trade_sizes = metrics.trade_sizes
    if not trade_sizes or len(trade_sizes) < 3:
        return 0.0

    # Analyze trade size patterns
    # Check if gradually increasing (smart accumulation)
    recent_sizes = trade_sizes[-5:] if len(trade_sizes) >= 5 else trade_sizes
    size_trend = 0.0

    for i in range(1, len(recent_sizes)):
        if recent_sizes[i] > recent_sizes[i-1]:
            size_trend += 1
        elif recent_sizes[i] < recent_sizes[i-1]:
            size_trend -= 1

    # Normalize to [-1, 1]
    if len(recent_sizes) > 1:
        size_trend /= (len(recent_sizes) - 1)

    # Positive gradual increase = smart accumulation
    if size_trend > 0.3 and size_trend < 0.8:
        score += 0.4

    # Check for pyramid-up behavior (adding to winners)
    if hasattr(metrics, 'roi_7d') and metrics.roi_7d and metrics.roi_7d > 0:
        if size_trend > 0.2:
            score += 0.3

    # Check for average-down behavior (adding to losers)
    if hasattr(metrics, 'roi_7d') and metrics.roi_7d and metrics.roi_7d < 0:
        if size_trend > 0.3:
            score -= 0.2  # Penalty for averaging down

    # Check for FOMO behavior (large sudden positions)
    if len(recent_sizes) >= 2:
        size_variance = max(recent_sizes) - min(recent_sizes)
        avg_size = sum(recent_sizes) / len(recent_sizes)
        if avg_size > 0:
            cv = size_variance / avg_size  # Coefficient of variation
            if cv > 2.0:  # High variance = impulsive trading
                score -= 0.3

    return max(0.0, min(1.0, score))


def _detect_market_regime(metrics) -> str:
    """
    Detect market regime based on wallet performance patterns.

    Returns: "BULL", "BEAR", "VOLATILE", or "NEUTRAL"
    """
    if not hasattr(metrics, 'roi_7d') or not hasattr(metrics, 'roi_30d'):
        return "NEUTRAL"

    roi_7d = metrics.roi_7d
    roi_30d = metrics.roi_30d
    volatility = getattr(metrics, 'volatility_30d', None)

    # Bull market: Strong positive returns across timeframes
    if roi_30d and roi_30d > 20 and roi_7d and roi_7d > 10:
        if volatility and volatility < 30:
            return "BULL"

    # Bear market: Negative returns or weak performance
    if roi_30d and roi_30d < -10:
        return "BEAR"
    if roi_7d and roi_7d < 0 and roi_30d and roi_30d < 5:
        return "BEAR"

    # Volatile: High volatility with mixed returns
    if volatility and volatility > 50:
        if roi_7d and abs(roi_7d) > 20:
            return "VOLATILE"

    # Check for regime switches
    if roi_30d and roi_30d > 10 and roi_7d and roi_7d < roi_30d * 0.2:
        return "VOLATILE"  # Momentum breakdown

    if roi_30d and roi_30d < 0 and roi_7d and roi_7d > 10:
        return "BULL"  # Recovery (early bull)

    return "NEUTRAL"


def _apply_archetype_adjustments(tracker: ScoreTracker, metrics, regime: str) -> None:
    """
    Apply archetype-specific adjustments based on trading patterns.

    Different trading styles excel in different market conditions:
    - Scalpers: Short-term trades, excel in volatile markets
    - Swing traders: Medium-term holds, excel in trending markets
    - Whales: Large positions, excel in stable markets
    """
    # Detect archetype from trade patterns
    avg_hold_time = getattr(metrics, 'avg_hold_time_hours', 24) or 24
    trade_freq = getattr(metrics, 'trade_count_30d', 30) or 30
    avg_size = getattr(metrics, 'avg_trade_size_sol', 1.0) or 1.0

    # Classify archetype
    if avg_hold_time < 1 and trade_freq > 100:
        archetype = "SCALPER"
    elif avg_hold_time < 24 and trade_freq > 50:
        archetype = "DAY_TRADER"
    elif avg_hold_time < 168 and trade_freq > 20:
        archetype = "SWING_TRADER"
    elif avg_size > 10 and trade_freq < 20:
        archetype = "WHALE"
    else:
        archetype = "GENERAL"

    # Apply regime-specific adjustments
    if regime == "VOLATILE":
        if archetype in ["SCALPER", "DAY_TRADER"]:
            tracker.add_pos("regime_adjustment", 5.0)
        elif archetype == "SWING_TRADER":
            tracker.add_pos("regime_adjustment", 3.0)
        elif archetype == "WHALE":
            tracker.add_neg("regime_adjustment", 3.0)  # Whales struggle in volatility

    elif regime == "BULL":
        if archetype == "SWING_TRADER":
            tracker.add_pos("regime_adjustment", 5.0)
        elif archetype == "WHALE":
            tracker.add_pos("regime_adjustment", 3.0)
        elif archetype in ["SCALPER", "DAY_TRADER"]:
            tracker.add_neg("regime_adjustment", 2.0)  # May miss bigger moves

    elif regime == "BEAR":
        if archetype == "SCALPER":
            tracker.add_pos("regime_adjustment", 5.0)  # Quick exits good in bear markets
        elif archetype == "WHALE":
            tracker.add_neg("regime_adjustment", 5.0)  # Large positions risky in bear
        elif archetype == "SWING_TRADER":
            tracker.add_neg("regime_adjustment", 3.0)


def _calculate_enhanced_momentum_score(metrics) -> float:
    """
    Calculate enhanced momentum score with multiple indicators.

    Returns: Score from 0.0 to 1.0
    """
    if not metrics.roi_7d or not metrics.roi_30d:
        return 0.0

    score = 0.0
    roi_7d = metrics.roi_7d
    roi_30d = metrics.roi_30d

    # Base momentum: 7d vs 30d ratio
    if roi_30d > 0:
        momentum_ratio = roi_7d / roi_30d

        # Strong acceleration
        if momentum_ratio > 0.8:
            score += 0.4
        elif momentum_ratio > 0.6:
            score += 0.3
        elif momentum_ratio > 0.4:
            score += 0.2

        # Explosive growth (moonshot detection)
        if roi_7d > 100:
            score += 0.3
        elif roi_7d > 50:
            score += 0.2
        elif roi_7d > 20:
            score += 0.1

        # Recent performance bonus
        if roi_7d > roi_30d * 1.2:  # Recent significantly outperforming
            score += 0.2

    # Recovery bonus (turning around)
    elif roi_30d < 0 and roi_7d > 10:
        score += 0.3  # Recovery momentum

    # Decline penalty
    elif roi_7d < roi_30d * 0.3:
        score -= 0.2

    return max(0.0, min(1.0, score))


def _get_current_weights() -> Dict[str, float]:
    """Load adaptive WQS weights from cache file, falling back to defaults (all 1.0)."""
    try:
        from .adaptive_weights import get_effective_wqs_weights
        return get_effective_wqs_weights()
    except ImportError:
        return {}


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
    roi_90d: Optional[float] = None  # 90-day ROI for multi-timeframe recovery check
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
    total_unrealized_gain_sol: Optional[float] = None  # Paper gains from profitable open positions
    dex_diversity_score: Optional[int] = None  # Count of unique DEXs used
    uses_limit_orders: bool = False  # Detected Jupiter Limit Order usage
    uses_mev_protection: bool = False  # Detected Jito bundle/MEV protection usage
    correlated_with_scam: bool = False  # Wallet or funder on known scam denylist
    unique_token_categories: Optional[int] = None  # Count of unique token categories traded
    mev_risk_score: Optional[float] = None  # Fraction of trades appearing in sandwich blocks (0.0-1.0)
    archetype: Optional[str] = None  # Trader archetype (SCALPER, SWING, WHALE, SNIPER, INSIDER)
    trajectory: Optional[str] = None  # Multi-timeframe trajectory (IMPROVING, STABLE, DECLINING, PEAKED)
    volatility_30d: Optional[float] = None  # Volatility measure over 30 days
    trade_sizes: Optional[list] = None  # List of trade sizes for pattern detection
    avg_hold_time_hours: Optional[float] = None  # Average position hold time in hours


@dataclass
class RawScoreComponents:
    """Separated bonus and penalty contributions for confidence-aware scoring."""
    positive: float = 0.0
    negative: float = 0.0   # stored as absolute value (positive number)
    is_instant_reject: bool = False
    components: Dict[str, float] = None  # type: ignore

    def __post_init__(self):
        if self.components is None:
            self.components = {}

    @property
    def raw_score(self) -> float:
        """Traditional combined score (positive - negative), clamped to [0, 100]."""
        return max(0.0, min(self.positive - self.negative, 100.0))

    @property
    def components_json(self) -> str:
        """Serialize components dict to JSON string for database storage."""
        import json
        return json.dumps(self.components)


def _calculate_raw_score(metrics: WalletMetrics, strategy: str = "SHIELD") -> RawScoreComponents:
    """
    Compute bonus and penalty components of the raw quality score (0-100).

    Separates positive contributions (bonuses) from negative ones (penalties)
    so that the confidence multiplier can be applied to bonuses only,
    while penalties retain full weight regardless of sample size.

    When strategy="SPEAR", weights are shifted toward upside capture
    and away from drawdown conservatism, matching Spear's higher risk appetite.

    Returns RawScoreComponents with is_instant_reject=True for categorically
    uncopyable wallets (snipers) that bypass the normal scoring pipeline.
    """
    _is_spear = strategy.upper() == "SPEAR"
    tracker = ScoreTracker()

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
        if roi_30d < 1.0 and roi_7d > 10.0:
            _is_pump_spike = True  # Near-zero baseline + any meaningful spike = suspicious
        else:
            _is_pump_spike = roi_7d > max(roi_30d * 2.0, 5.0)
    else:
        _is_pump_spike = roi_7d > max(abs(roi_30d) * 3.0, 15.0) and roi_7d > 50

    if _use_recency and not _is_pump_spike and roi_30d >= 1.0 and roi_7d > 0:
        # Standard 30d ROI contribution
        base_30d = min(25.0, (roi_30d / 100.0) * 25.0)
        # Time-weighted ROI: blends recent (7d) and full-month (30d)
        weighted_roi = roi_7d * 0.5 + roi_30d * 0.5
        recency_score = min(25.0, (weighted_roi / 100.0) * 25.0)
        # Use the better of standard and recency-weighted, so recency only helps
        tracker.add_pos("roi_score", max(base_30d, recency_score))
        # Bonus for wallets where recent > monthly (upward momentum confirmed)
        if roi_30d >= 1.0 and roi_7d > roi_30d * 0.6:
            tracker.add_pos("roi_score", 5.0)
    else:
        if roi_30d > 0:
            tracker.add_pos("roi_score", min(25.0, (roi_30d / 100.0) * 25.0))

    if roi_7d > 0 and not _is_pump_spike:
        tracker.add_pos("roi_score", min(10.0, (roi_7d / 100.0) * 10.0))

    # Consistency Bonus: 7d is positive defined as > -5% (allow small pullback)
    # and 30d is solid.
    if roi_7d > -5.0 and roi_30d > 20.0:
        tracker.add_pos("roi_score", 10.0)

    # 2) Win Rate & Profit Factor (0-20 pts)
    # Win-rate bonuses above +10 require sound profit factor to prevent
    # Martingale wallets (high win rate, catastrophic losses) from passing.
    win_rate = metrics.win_rate or 0.0
    profit_factor = metrics.profit_factor

    if win_rate >= 0.5:
        tracker.add_pos("win_rate_score", 5.0)
    if win_rate >= 0.65:
        tracker.add_pos("win_rate_score", 5.0)
    # Tiers above +10 require profit_factor >= 1.2 to gate Martingale risk
    if win_rate >= 0.80 and (profit_factor is None or profit_factor >= 1.2):
        tracker.add_pos("win_rate_score", 5.0)
    if win_rate >= 0.90 and (profit_factor is None or profit_factor >= 1.2):
        tracker.add_pos("win_rate_score", 5.0)
        
            
    # 3) Activity Level (0-20 pts)
    count = metrics.trade_count_30d or 0
    
    # Monotonic increase up to saturation
    if count >= 5:
        tracker.add_pos("activity_score", 2.0)
    if count >= 10:
        tracker.add_pos("activity_score", 3.0)
    if count >= 20:
        tracker.add_pos("activity_score", 5.0)
    if count >= 50:
        tracker.add_pos("activity_score", 5.0)
    if count >= 100:
        tracker.add_pos("activity_score", 5.0)
    
    # 4) Penalties (Drawdown & Pump-Dump)
    dd = metrics.max_drawdown_30d or 0.0

    # Linear drawdown penalty: -0.2 points per percent of drawdown
    tracker.add_neg("drawdown_penalty", dd * 0.2)

    # D3: Recovery fragility penalty — positive 30d ROI on a wallet with
    # negative 90d ROI suggests the recent gains are a recovery, not an edge.
    if metrics.roi_90d is not None and metrics.roi_90d < 0 and (metrics.roi_30d or 0) > 0:
        tracker.add_neg("recovery_fragility", 10.0)

    # Anti-Pump-and-Dump / Lucky Shot Check
    if _is_pump_spike:
        tracker.add_neg("pump_spike_penalty", 25.0)
        
    # 5) Scalability / Liquidity Safety (Implicit in avg_trade_size)
    if (metrics.avg_trade_size_sol or 0) < 0.05:
        tracker.add_neg("pump_spike_penalty", 10.0)

    # 6) Consistency (Win Streak)
    if metrics.win_streak_consistency and metrics.win_streak_consistency > 0.4:
        tracker.add_pos("consistency_score", 5.0)

    # 7) Sniper / Bot Penalty (Critical for Copy Trading)
    if metrics.avg_entry_delay_seconds is not None:
        if metrics.avg_entry_delay_seconds < 30:
            return tracker.to_components(is_instant_reject=True)
        
        elif metrics.avg_entry_delay_seconds < 60:
            tracker.add_neg("sniper_penalty", 15.0)
            
        elif 120 < metrics.avg_entry_delay_seconds < 3600:
            tracker.add_pos("entry_delay_score", 15.0)

    # ---------------------------------------------------------
    # IMPROVED: Profit Factor Logic (Martingale Risk Detection)
    # ---------------------------------------------------------
    if metrics.profit_factor is not None:
        if metrics.profit_factor > 3.0:
            tracker.add_pos("pf_score", 15.0)
        elif metrics.profit_factor > 1.5:
            tracker.add_pos("pf_score", 5.0)
        elif metrics.profit_factor >= 1.2:
            tracker.add_pos("pf_score", 2.0)
        elif metrics.profit_factor >= 1.15:
            tracker.add_neg("pf_score", 1.0)
        elif metrics.profit_factor >= 1.1:
            tracker.add_neg("pf_score", 3.0)
        elif metrics.profit_factor >= 1.0:
            tracker.add_neg("pf_score", 6.0)
        elif metrics.profit_factor >= 0.5:
            tracker.add_neg("pf_score", 25.0)
        else:
            tracker.add_neg("pf_score", 40.0)
    
    # Unproven wallet penalty (continuous based on parse rate when available)
    if metrics.parse_rate is not None:
        if metrics.parse_rate < 0.60:
            continuous_penalty = (0.60 - metrics.parse_rate) * 80.0
            tracker.add_neg("martingale_penalty", continuous_penalty)
    elif metrics.is_unproven:
        tracker.add_neg("martingale_penalty", 20.0)

    # Explicit Martingale Pattern Detection
    if profit_factor is not None and win_rate > 0.70 and profit_factor < 1.5:
        tracker.add_neg("martingale_penalty", 15.0)

    # 2a: Profit-Factor-to-Win-Rate Ratio Signal
    if win_rate > 0.70 and profit_factor is not None and profit_factor > 0:
        pf_wr_ratio = profit_factor / win_rate
        if pf_wr_ratio < 1.3:
            tracker.add_neg("pf_wr_penalty", 20.0)

    # 8) Composite Risk-Adjusted Return (Sortino + Drawdown)
    sortino = metrics.sortino_ratio
    if sortino is not None:
        if sortino >= 3.0 and dd < 10.0:
            tracker.add_pos("sortino_score", 20.0)
        elif sortino >= 2.0 and dd < 20.0:
            tracker.add_pos("sortino_score", 15.0)
        elif sortino >= 1.0 and dd < 30.0:
            tracker.add_pos("sortino_score", 10.0)
        elif sortino < 0.5 and dd > 40.0:
            tracker.add_neg("sortino_score", 15.0)
        elif sortino < 0:
            tracker.add_neg("sortino_score", 10.0)

    # Phase 3c: Per-strategy weight adjustments
    if _is_spear:
        if sortino is not None and sortino >= 1.5:
            tracker.add_pos("sortino_score", 5.0)
    else:
        if dd < 5.0:
            tracker.add_pos("sortino_score", 5.0)
    
    # 9) Insider / Fresh Wallet Penalty
    if metrics.is_fresh_wallet:
        tracker.add_neg("insider_penalty", 10.0)

    # D5: Scam correlation penalty
    if metrics.correlated_with_scam:
        tracker.add_neg("scam_penalty", 20.0)

    # Phase 5b: MEV/Sandwich Risk Penalty
    if metrics.mev_risk_score is not None and metrics.mev_risk_score > 0.05:
        if metrics.mev_risk_score > 0.50:
            tracker.add_neg("mev_risk_penalty", 25.0)
        elif metrics.mev_risk_score > 0.25:
            tracker.add_neg("mev_risk_penalty", 15.0)
        elif metrics.mev_risk_score > 0.10:
            tracker.add_neg("mev_risk_penalty", 8.0)
    
    # 10) Smart Money Bonuses
    if metrics.dex_diversity_score is not None and metrics.dex_diversity_score >= 3:
        tracker.add_pos("dex_diversity_score", 5.0)

    if metrics.uses_limit_orders:
        tracker.add_pos("smart_money_score", 10.0)

    if metrics.uses_mev_protection:
        tracker.add_pos("smart_money_score", 10.0)

    # Token Category Concentration (Phase 2c)
    if metrics.unique_token_categories is not None:
        if metrics.unique_token_categories >= 3:
            tracker.add_pos("token_diversity_score", 5.0)
        elif metrics.unique_token_categories == 1:
            tracker.add_neg("token_diversity_score", 5.0)

    should_remove_bonuses = False
    if metrics.roi_30d is not None and metrics.roi_30d < -10:
        should_remove_bonuses = True
    if metrics.profit_factor is not None and metrics.profit_factor < 1.2:
        should_remove_bonuses = True
    if metrics.win_rate is not None and metrics.win_rate < 0.45:
        should_remove_bonuses = True

    if should_remove_bonuses:
        if metrics.dex_diversity_score is not None and metrics.dex_diversity_score >= 3:
            tracker.add_neg("smart_money_removal", 5.0)
        if metrics.uses_limit_orders:
            tracker.add_neg("smart_money_removal", 10.0)
        if metrics.uses_mev_protection:
            tracker.add_neg("smart_money_removal", 10.0)

    # Pattern Recognition: Smart Accumulation Detection
    accumulation_score = _detect_smart_accumulation(metrics)
    if accumulation_score > 0.6:
        tracker.add_pos("smart_accumulation", 8.0)
    elif accumulation_score > 0.4:
        tracker.add_pos("smart_accumulation", 5.0)
    elif accumulation_score < 0.2:
        tracker.add_neg("smart_accumulation", 5.0)

    # Pattern Recognition: Enhanced Momentum Detection
    momentum_score = _calculate_enhanced_momentum_score(metrics)
    if momentum_score > 0.7:
        tracker.add_pos("enhanced_momentum", 10.0)
    elif momentum_score > 0.5:
        tracker.add_pos("enhanced_momentum", 7.0)
    elif momentum_score > 0.3:
        tracker.add_pos("enhanced_momentum", 5.0)
    elif momentum_score < 0.1:
        tracker.add_neg("enhanced_momentum", 3.0)

    # Pattern Recognition: Market Regime Detection and Archetype Adjustments
    market_regime = _detect_market_regime(metrics)
    _apply_archetype_adjustments(tracker, metrics, market_regime)

    # Regime-specific adjustments
    if market_regime == "BULL":
        tracker.add_pos("market_regime", 3.0)
    elif market_regime == "BEAR":
        tracker.add_neg("market_regime", 2.0)
    elif market_regime == "VOLATILE":
        # Neutral in volatile markets, but add bonus for adaptability
        if metrics.volatility_30d and metrics.volatility_30d > 50:
            if metrics.win_rate and metrics.win_rate > 0.5:
                tracker.add_pos("adaptability", 5.0)

    # 11) Bag Holder Penalty (The "Hidden Loser" Detector)
    if metrics.total_unrealized_loss_sol is not None and metrics.total_realized_profit_sol is not None:
        if metrics.total_realized_profit_sol > 0:
            loss_ratio = metrics.total_unrealized_loss_sol / metrics.total_realized_profit_sol
            tracker.add_neg("martingale_penalty", min(30.0, loss_ratio * 60.0))
        elif metrics.total_unrealized_loss_sol > 0:
            tracker.add_neg("martingale_penalty", 20.0)
    
    # 11b) Unproven Edge Penalty (D2: Paper vs Realized Gain Ratio)
    # Flag wallets where paper gains exceed 60% of total gains as unproven.
    if metrics.total_unrealized_gain_sol is not None and metrics.total_unrealized_gain_sol > 0:
        total_gains = (metrics.total_realized_profit_sol or 0) + metrics.total_unrealized_gain_sol
        if total_gains > 0:
            paper_ratio = metrics.total_unrealized_gain_sol / total_gains
            if paper_ratio > 0.60:
                tracker.add_neg("martingale_penalty", 15.0)
    
    # 11) Recency Bias (Freshness)
    if metrics.last_trade_at:
        try:
            last_trade_str = metrics.last_trade_at.replace("Z", "+00:00")
            last_trade = datetime.fromisoformat(last_trade_str)
            now = datetime.now(timezone.utc)
            if last_trade.tzinfo is None:
                now = now.replace(tzinfo=None)
            days_since_trade = (now - last_trade).days
            
            if days_since_trade <= 2:
                tracker.add_pos("recency_score", 10.0)
            elif days_since_trade <= 5:
                tracker.add_pos("recency_score", 5.0)
            elif days_since_trade > 14:
                tracker.add_neg("recency_score", 10.0)
                
            # Momentum: use WMI instead of hardcoded thresholds
            wmi = _compute_wmi(roi_7d, roi_30d, count)
            if wmi > 0.5:
                tracker.add_pos("roi_score", 10.0)
            elif wmi > 0.2:
                tracker.add_pos("roi_score", 5.0)
            elif wmi < -0.5:
                tracker.add_neg("roi_score", 15.0)
            elif wmi < -0.2:
                tracker.add_neg("roi_score", 5.0)
        except (ValueError, TypeError):
            pass

    # Apply adaptive weights to component contributions
    # When calibration has data, some components get weighted up/down.
    # Default (all 1.0) means this is a no-op until PnL correlation data arrives.
    try:
        weights = _get_current_weights()
        for name, multiplier in weights.items():
            if name in tracker.components and multiplier != 1.0:
                tracker.components[name] *= multiplier
        tracker.positive = sum(v for v in tracker.components.values() if v > 0)
        tracker.negative = abs(sum(v for v in tracker.components.values() if v < 0))
    except Exception:
        pass

    # ---------------------------------------------------------
    # INTELLIGENT PENALTY CAPPING (Task 10)
    # ---------------------------------------------------------
    # Prevent excessive demotion by capping total penalties
    # and applying penalty precedence logic

    # Get configuration for penalty capping
    max_total_penalty = float(os.getenv("SCOUT_MAX_TOTAL_PENALTY", "40.0"))  # Max total penalty points
    penalty_cap_enabled = os.getenv("SCOUT_PENALTY_CAP_ENABLED", "true").lower() == "true"
    penalty_precedence_enabled = os.getenv("SCOUT_PENALTY_PRECEDENCE", "true").lower() == "true"

    if penalty_cap_enabled and tracker.negative > 0:
        # Calculate total negative before capping
        total_negative_before = tracker.negative
        total_positive = tracker.positive

        # Apply total penalty cap if negative would exceed threshold
        if tracker.negative > max_total_penalty:
            excess = tracker.negative - max_total_penalty

            # Scale down individual penalties proportionally
            if tracker.negative > 0:
                scale_factor = max_total_penalty / tracker.negative

                # Scale each negative component
                for name in list(tracker.components.keys()):
                    if tracker.components[name] < 0:
                        tracker.components[name] *= scale_factor

                # Recalculate negative total
                tracker.negative = max_total_penalty

                logger.debug(
                    f"Penalty cap applied: {total_negative_before:.1f} -> {tracker.negative:.1f} "
                    f"(excess {excess:.1f} capped at {max_total_penalty})"
                )

        # Apply penalty precedence (only keep most severe penalty per category)
        if penalty_precedence_enabled:
            tracker._apply_penalty_precedence()

        # Apply penalty confidence weighting
        # (uncertain penalties count less)
        tracker._apply_penalty_confidence()

    return tracker.to_components()


def calculate_wqs(metrics: WalletMetrics, strategy: str = "SHIELD") -> float:
    """
    Calculate Wallet Quality Score (WQS) v2 (0-100).

    Returns the RAW quality score WITHOUT confidence weighting.
    Confidence is unbundled — use calculate_wqs_with_confidence() to get
    both raw score and sample confidence separately.

    Scoring breakdown:
    - ROI performance: up to 25 points (capped at 100% ROI)
    - Consistency: up to 25 points (win_streak_consistency)
    - Win rate fallback: up to 25 points (if consistency unavailable)
    - Activity bonus: +5 points if trade_count_30d >= 50
    - Anti-pump-and-dump: -25 points if 7d ROI > 2x 30d ROI (and 30d ROI > 0)
    - Drawdown penalty: -0.2 * drawdown_percent

    Penalties retain full weight regardless of sample size, so wallets
    with suspicious patterns (pump-and-dump, high drawdown, bag-holding)
    are penalized regardless of confidence.

    Args:
        metrics: WalletMetrics object with wallet data
        strategy: "SHIELD" (conservative) or "SPEAR" (aggressive)

    Returns:
        Raw WQS score from 0 to 100 (UN-weighted by confidence)
    """
    components = _calculate_raw_score(metrics, strategy=strategy)
    if components.is_instant_reject:
        return 0.0

    return components.raw_score


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


def calculate_wqs_with_confidence(metrics: WalletMetrics, strategy: str = "SHIELD") -> WqsResult:
    """
    Like calculate_wqs() but returns quality score and sample confidence separately.

    Use when the caller needs to distinguish between a wallet that is *bad* vs one
    that is *unproven* — they both produce a low adjusted_score but for different
    reasons, and the Operator's position sizer handles them differently.
    """
    components = _calculate_raw_score(metrics, strategy=strategy)
    if components.is_instant_reject:
        return WqsResult(score=0.0, confidence=0.0, adjusted_score=0.0)

    trade_count = metrics.trade_count_30d or 0
    confidence = _compute_confidence(trade_count, metrics.profit_factor, metrics, metrics.is_unproven)
    adjusted_score = max(0.0, min(components.positive * confidence - components.negative, 100.0))

    adjusted_score = max(0.0, min(adjusted_score, 100.0))
    return WqsResult(score=components.raw_score, confidence=confidence, adjusted_score=adjusted_score)


def classify_wallet(
    wqs_score: float,
    active_threshold: float = 65.0,
    candidate_threshold: float = 20.0,
    confidence: Optional[float] = None,
    min_confidence: float = 0.70,
) -> str:
    """
    Classify wallet based on WQS score and optional sample confidence.

    When confidence is provided, ACTIVE status requires BOTH:
    - wqs_score >= active_threshold
    - confidence >= min_confidence

    Args:
        wqs_score: Computed WQS (0-100) raw score
        active_threshold: Min score for ACTIVE status (default 65.0)
        candidate_threshold: Min score for CANDIDATE status (default 20.0)
        confidence: Sample confidence 0.0-1.0 (optional)
        min_confidence: Minimum confidence for ACTIVE status (default 0.70)

    Returns:
        'ACTIVE', 'CANDIDATE', or 'REJECTED'
    """
    if wqs_score >= active_threshold:
        if confidence is not None and confidence < min_confidence:
            return "CANDIDATE"
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
