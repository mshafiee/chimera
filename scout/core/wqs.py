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
from enum import Enum, auto
from typing import Optional, Dict, Union, List, Tuple, Any
from datetime import datetime

from .utils import utcnow

from decimal import Decimal

import os
import logging

logger = logging.getLogger(__name__)


class PenaltyCategory(Enum):
    MARTINGALE = auto()
    PUMP_SPIKE = auto()
    SNIPER = auto()
    DRAWDOWN = auto()
    PF_WR = auto()
    MEV_RISK = auto()
    SCAM = auto()
    INSIDER = auto()
    SMART_MONEY = auto()
    CVAR = auto()                      # Conditional Value at Risk
    DRAWDOWN_DURATION = auto()         # Time to recover from drawdown
    ULCER_INDEX = auto()              # Combined depth + duration metric


_STRING_TO_PENALTY: Dict[str, PenaltyCategory] = {
    'martingale_penalty': PenaltyCategory.MARTINGALE,
    'pump_spike_penalty': PenaltyCategory.PUMP_SPIKE,
    'sniper_penalty': PenaltyCategory.SNIPER,
    'drawdown_penalty': PenaltyCategory.DRAWDOWN,
    'pf_wr_penalty': PenaltyCategory.PF_WR,
    'mev_risk_penalty': PenaltyCategory.MEV_RISK,
    'scam_penalty': PenaltyCategory.SCAM,
    'insider_penalty': PenaltyCategory.INSIDER,
    'smart_money_removal': PenaltyCategory.SMART_MONEY,
    'cvar_penalty': PenaltyCategory.CVAR,
    'drawdown_duration_penalty': PenaltyCategory.DRAWDOWN_DURATION,
    'ulcer_index_penalty': PenaltyCategory.ULCER_INDEX,
}


class ScoreTracker:
    """Tracks per-component score contributions for adaptive weight calibration."""
    def __init__(self):
        self.positive = 0.0
        self.negative = 0.0
        self.components: Dict[Union[str, PenaltyCategory], float] = {}

    def add_pos(self, name: str, amount: float) -> None:
        self.positive += amount
        self.components[name] = self.components.get(name, 0.0) + amount

    def add_neg(self, category: Union[str, PenaltyCategory], amount: float) -> None:
        self.negative += amount
        self.components[category] = self.components.get(category, 0.0) - amount

    def to_components(self, is_instant_reject: bool = False) -> "RawScoreComponents":
        converted: Dict[str, float] = {}
        for cat, val in self.components.items():
            key = cat.name.lower() if isinstance(cat, PenaltyCategory) else cat
            converted[key] = val
        return RawScoreComponents(
            positive=self.positive,
            negative=self.negative,
            is_instant_reject=is_instant_reject,
            components=converted,
        )

    def _apply_penalty_precedence(self) -> None:
        penalty_categories: Dict[PenaltyCategory, List[Tuple[PenaltyCategory, float]]] = {
            cat: [] for cat in PenaltyCategory
        }
        for category, value in self.components.items():
            if isinstance(category, PenaltyCategory) and value < 0:
                penalty_categories[category].append((category, value))
        for cat, penalties in penalty_categories.items():
            if len(penalties) > 1:
                penalties.sort(key=lambda x: abs(x[1]), reverse=True)
                most_severe_name, most_severe_value = penalties[0]
                for other_name, _ in penalties[1:]:
                    if other_name in self.components:
                        del self.components[other_name]
                self.components[most_severe_name] = most_severe_value

    def _apply_penalty_confidence(self) -> None:
        penalty_confidence = {
            PenaltyCategory.MARTINGALE: 0.8,
            PenaltyCategory.PUMP_SPIKE: 0.9,
            PenaltyCategory.SNIPER: 1.0,
            PenaltyCategory.DRAWDOWN: 0.9,
            PenaltyCategory.PF_WR: 0.7,
            PenaltyCategory.MEV_RISK: 0.6,
            PenaltyCategory.SCAM: 1.0,
            PenaltyCategory.INSIDER: 0.8,
            PenaltyCategory.SMART_MONEY: 0.5,
        }
        for category, value in list(self.components.items()):
            if isinstance(category, PenaltyCategory) and value < 0:
                conf = penalty_confidence.get(category, 0.5)
                if conf < 0.8:
                    adjusted_value = value * (0.5 + 0.5 * conf)
                    self.components[category] = adjusted_value
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

    if not hasattr(metrics, 'trade_sizes') or not metrics.trade_sizes:
        return 0.0

    trade_sizes = metrics.trade_sizes
    if not trade_sizes or len(trade_sizes) < 3:
        return 0.0

    recent_sizes = trade_sizes[-5:] if len(trade_sizes) >= 5 else trade_sizes
    size_trend = 0.0

    for i in range(1, len(recent_sizes)):
        if recent_sizes[i] > recent_sizes[i-1]:
            size_trend += 1
        elif recent_sizes[i] < recent_sizes[i-1]:
            size_trend -= 1

    if len(recent_sizes) > 1:
        size_trend /= (len(recent_sizes) - 1)

    if size_trend > 0.3 and size_trend < 0.8:
        score += 0.4

    if hasattr(metrics, 'roi_7d') and metrics.roi_7d and metrics.roi_7d > 0:
        if size_trend > 0.2:
            score += 0.3

    if hasattr(metrics, 'roi_7d') and metrics.roi_7d and metrics.roi_7d < 0:
        if size_trend > 0.3:
            score -= 0.2

    if len(recent_sizes) >= 2:
        size_variance = max(recent_sizes) - min(recent_sizes)
        avg_size = sum(recent_sizes) / len(recent_sizes)
        if avg_size > 0:
            cv = size_variance / avg_size
            if cv > 2.0:
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

    if roi_30d and roi_30d > 20 and roi_7d and roi_7d > 10:
        if volatility and volatility < 30:
            return "BULL"

    if roi_30d and roi_30d < -10:
        return "BEAR"
    if roi_7d and roi_7d < 0 and roi_30d and roi_30d < 5:
        return "BEAR"

    if volatility and volatility > 50:
        if roi_7d and abs(roi_7d) > 20:
            return "VOLATILE"

    if roi_30d and roi_30d > 10 and roi_7d and roi_7d < roi_30d * 0.2:
        return "VOLATILE"

    if roi_30d and roi_30d < 0 and roi_7d and roi_7d > 10:
        return "BULL"

    return "NEUTRAL"


def _apply_archetype_adjustments(tracker: ScoreTracker, metrics, regime: str) -> None:
    """
    Apply archetype-specific adjustments based on trading patterns.

    Different trading styles excel in different market conditions:
    - Scalpers: Short-term trades, excel in volatile markets
    - Swing traders: Medium-term holds, excel in trending markets
    - Whales: Large positions, excel in stable markets
    """
    avg_hold_time = getattr(metrics, 'avg_hold_time_hours', 24) or 24
    trade_freq = getattr(metrics, 'trade_count_30d', 30) or 30
    avg_size = float(getattr(metrics, 'avg_trade_size_sol', Decimal(1)) or Decimal(1))

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

    if regime == "VOLATILE":
        if archetype in ["SCALPER", "DAY_TRADER"]:
            tracker.add_pos("regime_adjustment", 5.0)
        elif archetype == "SWING_TRADER":
            tracker.add_pos("regime_adjustment", 3.0)
        elif archetype == "WHALE":
            tracker.add_neg("regime_adjustment", 3.0)

    elif regime == "BULL":
        if archetype == "SWING_TRADER":
            tracker.add_pos("regime_adjustment", 5.0)
        elif archetype == "WHALE":
            tracker.add_pos("regime_adjustment", 3.0)
        elif archetype in ["SCALPER", "DAY_TRADER"]:
            tracker.add_neg("regime_adjustment", 2.0)

    elif regime == "BEAR":
        if archetype == "SCALPER":
            tracker.add_pos("regime_adjustment", 5.0)
        elif archetype == "WHALE":
            tracker.add_neg("regime_adjustment", 5.0)
        elif archetype == "SWING_TRADER":
            tracker.add_neg("regime_adjustment", 3.0)


def _calculate_enhanced_momentum_score(metrics) -> float:
    """
    Calculate enhanced momentum score with multiple indicators.

    Returns: Score from 0.0 to 1.0
    """
    if metrics.roi_7d is None or metrics.roi_30d is None:
        return 0.0

    score = 0.0
    roi_7d = float(metrics.roi_7d)
    roi_30d = float(metrics.roi_30d)

    if roi_30d > 0:
        momentum_ratio = roi_7d / roi_30d

        if momentum_ratio > 0.8:
            score += 0.4
        elif momentum_ratio > 0.6:
            score += 0.3
        elif momentum_ratio > 0.4:
            score += 0.2

        if roi_7d > 100:
            score += 0.3
        elif roi_7d > 50:
            score += 0.2
        elif roi_7d > 20:
            score += 0.1

        if roi_7d > roi_30d * 1.2:
            score += 0.2

    elif roi_30d < 0 and roi_7d > 10:
        score += 0.3

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
    roi_90d: Optional[float] = None
    trade_count_30d: Optional[int] = None
    win_rate: Optional[float] = None
    max_drawdown_30d: Optional[float] = None
    avg_trade_size_sol: Optional[Decimal] = None
    last_trade_at: Optional[str] = None
    win_streak_consistency: Optional[float] = None
    avg_entry_delay_seconds: Optional[float] = None
    profit_factor: Optional[float] = None
    sortino_ratio: Optional[float] = None
    is_fresh_wallet: bool = False
    is_unproven: bool = False
    parse_rate: Optional[float] = None
    total_unrealized_loss_sol: Optional[Decimal] = None
    total_realized_profit_sol: Optional[Decimal] = None
    total_unrealized_gain_sol: Optional[Decimal] = None
    dex_diversity_score: Optional[int] = None
    uses_limit_orders: bool = False
    uses_mev_protection: bool = False
    correlated_with_scam: bool = False
    unique_token_categories: Optional[int] = None
    mev_risk_score: Optional[float] = None
    archetype: Optional[str] = None
    trajectory: Optional[str] = None
    volatility_30d: Optional[float] = None
    trade_sizes: Optional[list] = None
    avg_hold_time_hours: Optional[float] = None
    advanced_risk_features: Optional[Dict[str, Any]] = None
    replay_data_gap_ratio: Optional[float] = None  # Ratio of SELL events with data gaps to total SELL events
    is_tg_bot_user: bool = False  # Flagged as Telegram bot user (≥50% of ≥10 swaps through bot router)
    round_trip_ratio: Optional[float] = None  # Ratio of round-trip swaps to total swaps (arbitrage detection)
    pumpfun_trade_ratio: Optional[float] = None  # Fraction of trades on pump.fun bonding-curve tokens (mint ends with "pump")


@dataclass
class RawScoreComponents:
    """Separated bonus and penalty contributions for confidence-aware scoring."""
    positive: float = 0.0
    negative: float = 0.0
    is_instant_reject: bool = False
    components: Dict[str, float] = None

    def __post_init__(self):
        if self.components is None:
            self.components = {}

    @property
    def raw_score(self) -> float:
        return max(0.0, min(self.positive - self.negative, 100.0))

    @property
    def components_json(self) -> str:
        import json
        return json.dumps(self.components)


def _calculate_raw_score(metrics: WalletMetrics, strategy: str = "SHIELD") -> RawScoreComponents:
    """
    Calculate raw WQS score with bonus and penalty components.

    NOTE: Advanced risk features (CVaR, drawdown duration, risk-adjusted ratios)
    from advanced_risk_features.py can be integrated here. To enable this:
    1. Pass raw trade history to this function (or pre-calculate in WalletMetrics)
    2. Extract advanced features: extract_advanced_risk_features(trade_history)
    3. Apply penalties: cvar_95 * 0.2, max_drawdown_duration * 0.1
    """
    _is_spear = strategy.upper() == "SPEAR"
    tracker = ScoreTracker()

    roi_7d = float(metrics.roi_7d) if metrics.roi_7d is not None else 0.0
    roi_30d = float(metrics.roi_30d) if metrics.roi_30d is not None else 0.0

    try:
        from config import ScoutConfig
        _use_recency = ScoutConfig.get_wqs_recency_weight() if ScoutConfig else True
    except ImportError:
        _use_recency = os.environ.get("SCOUT_WQS_RECENCY_WEIGHT", "true").lower() == "true"

    if roi_30d > 0:
        if roi_30d < 1.0 and roi_7d > 10.0:
            _is_pump_spike = True
        else:
            _is_pump_spike = roi_7d > max(roi_30d * 2.0, 5.0)
    else:
        _is_pump_spike = roi_7d > max(abs(roi_30d) * 3.0, 15.0) and roi_7d > 50

    if _use_recency and not _is_pump_spike and roi_30d >= 1.0 and roi_7d > 0:
        base_30d = min(25.0, (roi_30d / 100.0) * 25.0)
        weighted_roi = roi_7d * 0.5 + roi_30d * 0.5
        recency_score = min(25.0, (weighted_roi / 100.0) * 25.0)
        tracker.add_pos("roi_score", max(base_30d, recency_score))
        if roi_30d >= 1.0 and roi_7d > roi_30d * 0.6:
            tracker.add_pos("roi_score", 5.0)
    else:
        if roi_30d > 0:
            tracker.add_pos("roi_score", min(25.0, (roi_30d / 100.0) * 25.0))

    if roi_7d > 0 and not _is_pump_spike:
        tracker.add_pos("roi_score", min(10.0, (roi_7d / 100.0) * 10.0))

    if roi_7d > -5.0 and roi_30d > 20.0:
        tracker.add_pos("roi_score", 10.0)

    # Apply confidence penalty for wallets with FIFO replay data gaps
    # This reduces confidence in PnL-based scores when sell data is incomplete
    if metrics.replay_data_gap_ratio is not None and metrics.replay_data_gap_ratio > 0:
        gap_ratio = metrics.replay_data_gap_ratio
        # Apply penalty proportionally to ROI-based scores
        # Max penalty of 20 points when gap ratio is 100%
        roi_penalty = gap_ratio * 20.0
        if roi_penalty > 0:
            tracker.add_neg("replay_data_gap", roi_penalty)

    win_rate = metrics.win_rate or 0.0
    profit_factor = metrics.profit_factor

    if win_rate >= 0.5:
        tracker.add_pos("win_rate_score", 5.0)
    if win_rate >= 0.65:
        tracker.add_pos("win_rate_score", 5.0)
    if win_rate >= 0.80 and (profit_factor is None or profit_factor >= 1.2):
        tracker.add_pos("win_rate_score", 5.0)
    if win_rate >= 0.90 and (profit_factor is None or profit_factor >= 1.2):
        tracker.add_pos("win_rate_score", 5.0)
        
    count = metrics.trade_count_30d or 0
    
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
    
    dd = metrics.max_drawdown_30d or 0.0

    tracker.add_neg(PenaltyCategory.DRAWDOWN, dd * 0.2)

    if metrics.roi_90d is not None and metrics.roi_90d < 0 and (metrics.roi_30d or 0) > 0:
        tracker.add_neg("recovery_fragility", 10.0)

    if _is_pump_spike:
        tracker.add_neg(PenaltyCategory.PUMP_SPIKE, 25.0)
        
    if (metrics.avg_trade_size_sol or Decimal(0)) < Decimal('0.05'):
        tracker.add_neg(PenaltyCategory.PUMP_SPIKE, 10.0)

    # Pump.fun bonding-curve concentration check.
    # These tokens have $0 DEX liquidity — copy-trading is guaranteed to lose
    # from Jito tips. Hard-reject wallets that are predominantly pump.fun traders.
    if metrics.pumpfun_trade_ratio is not None:
        if metrics.pumpfun_trade_ratio > 0.5:
            tracker.add_neg("pumpfun_concentration", 100.0)
            return tracker.to_components(is_instant_reject=True)
        elif metrics.pumpfun_trade_ratio >= 0.3:
            tracker.add_neg("pumpfun_concentration", 15.0)

    if metrics.win_streak_consistency and metrics.win_streak_consistency > 0.4:
        tracker.add_pos("consistency_score", 5.0)

    if metrics.avg_entry_delay_seconds is not None:
        if metrics.avg_entry_delay_seconds < 30:
            return tracker.to_components(is_instant_reject=True)
        
        elif metrics.avg_entry_delay_seconds < 60:
            tracker.add_neg(PenaltyCategory.SNIPER, 15.0)
            
        elif 120 < metrics.avg_entry_delay_seconds < 3600:
            tracker.add_pos("entry_delay_score", 15.0)

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
    
    if metrics.parse_rate is not None:
        if metrics.parse_rate < 0.60:
            continuous_penalty = (0.60 - metrics.parse_rate) * 80.0
            tracker.add_neg(PenaltyCategory.MARTINGALE, continuous_penalty)
    elif metrics.is_unproven:
        tracker.add_neg(PenaltyCategory.MARTINGALE, 20.0)

    if profit_factor is not None and win_rate > 0.70 and profit_factor < 1.5:
        tracker.add_neg(PenaltyCategory.MARTINGALE, 15.0)

    if win_rate > 0.70 and profit_factor is not None and profit_factor > 0:
        pf_wr_ratio = profit_factor / win_rate
        if pf_wr_ratio < 1.3:
            tracker.add_neg(PenaltyCategory.PF_WR, 20.0)

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

    if _is_spear:
        if sortino is not None and sortino >= 1.5:
            tracker.add_pos("sortino_score", 5.0)
    else:
        if dd < 5.0:
            tracker.add_pos("sortino_score", 5.0)
    
    if metrics.is_fresh_wallet:
        tracker.add_neg(PenaltyCategory.INSIDER, 10.0)

    if metrics.correlated_with_scam:
        tracker.add_neg(PenaltyCategory.SCAM, 20.0)

    if metrics.mev_risk_score is not None and metrics.mev_risk_score > 0.05:
        if metrics.mev_risk_score > 0.50:
            tracker.add_neg(PenaltyCategory.MEV_RISK, 25.0)
        elif metrics.mev_risk_score > 0.25:
            tracker.add_neg(PenaltyCategory.MEV_RISK, 15.0)
        elif metrics.mev_risk_score > 0.10:
            tracker.add_neg(PenaltyCategory.MEV_RISK, 8.0)
    
    if metrics.dex_diversity_score is not None and metrics.dex_diversity_score >= 3:
        tracker.add_pos("dex_diversity_score", 5.0)

    if metrics.uses_limit_orders:
        tracker.add_pos("smart_money_score", 10.0)

    if metrics.uses_mev_protection:
        tracker.add_pos("smart_money_score", 10.0)

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
            tracker.add_neg(PenaltyCategory.SMART_MONEY, 5.0)
        if metrics.uses_limit_orders:
            tracker.add_neg(PenaltyCategory.SMART_MONEY, 10.0)
        if metrics.uses_mev_protection:
            tracker.add_neg(PenaltyCategory.SMART_MONEY, 10.0)

    accumulation_score = _detect_smart_accumulation(metrics)
    if accumulation_score > 0.6:
        tracker.add_pos("smart_accumulation", 8.0)
    elif accumulation_score > 0.4:
        tracker.add_pos("smart_accumulation", 5.0)
    elif accumulation_score < 0.2:
        tracker.add_neg("smart_accumulation", 5.0)

    momentum_score = _calculate_enhanced_momentum_score(metrics)
    if momentum_score > 0.7:
        tracker.add_pos("enhanced_momentum", 10.0)
    elif momentum_score > 0.5:
        tracker.add_pos("enhanced_momentum", 7.0)
    elif momentum_score > 0.3:
        tracker.add_pos("enhanced_momentum", 5.0)
    elif momentum_score < 0.1:
        tracker.add_neg("enhanced_momentum", 3.0)

    market_regime = _detect_market_regime(metrics)
    _apply_archetype_adjustments(tracker, metrics, market_regime)

    if market_regime == "BULL":
        tracker.add_pos("market_regime", 3.0)
    elif market_regime == "BEAR":
        tracker.add_neg("market_regime", 2.0)
    elif market_regime == "VOLATILE":
        if metrics.volatility_30d and metrics.volatility_30d > 50:
            if metrics.win_rate and metrics.win_rate > 0.5:
                tracker.add_pos("adaptability", 5.0)

    if metrics.total_unrealized_loss_sol is not None and metrics.total_realized_profit_sol is not None:
        if metrics.total_realized_profit_sol > Decimal(0):
            loss_ratio = float(metrics.total_unrealized_loss_sol) / float(metrics.total_realized_profit_sol)
            tracker.add_neg(PenaltyCategory.MARTINGALE, min(30.0, loss_ratio * 60.0))
        elif metrics.total_unrealized_loss_sol > Decimal(0):
            tracker.add_neg(PenaltyCategory.MARTINGALE, 20.0)
    
    if metrics.total_unrealized_gain_sol is not None and float(metrics.total_unrealized_gain_sol) > 0:
        total_gains = float(metrics.total_realized_profit_sol or 0) + float(metrics.total_unrealized_gain_sol)
        if total_gains > 0:
            paper_ratio = float(metrics.total_unrealized_gain_sol) / total_gains
            if paper_ratio > 0.60:
                tracker.add_neg(PenaltyCategory.MARTINGALE, 15.0)
    
    if metrics.last_trade_at:
        try:
            last_trade_str = metrics.last_trade_at.replace("Z", "+00:00")
            last_trade = datetime.fromisoformat(last_trade_str)
            now = utcnow()
            if last_trade.tzinfo is None:
                now = now.replace(tzinfo=None)
            days_since_trade = (now - last_trade).days
            
            if days_since_trade <= 2:
                tracker.add_pos("recency_score", 10.0)
            elif days_since_trade <= 5:
                tracker.add_pos("recency_score", 5.0)
            elif days_since_trade > 14:
                tracker.add_neg("recency_score", 10.0)
                
            wmi = _compute_wmi(roi_7d, roi_30d, count)
            if wmi > 0.5:
                tracker.add_pos("roi_score", 10.0)
            elif wmi > 0.2:
                tracker.add_pos("roi_score", 5.0)
            elif wmi < -0.5:
                tracker.add_neg("roi_score", 15.0)
            elif wmi < -0.2:
                tracker.add_neg("roi_score", 5.0)
        except (ValueError, TypeError) as e:
            logger.warning("Failed to parse last_trade_at for recency calculation: %s", e)

    try:
        weights = _get_current_weights()
        for name, multiplier in weights.items():
            key: Union[str, PenaltyCategory] = _STRING_TO_PENALTY.get(name, name)
            if key in tracker.components and multiplier != 1.0:
                tracker.components[key] *= multiplier
        tracker.positive = sum(v for v in tracker.components.values() if v > 0)
        tracker.negative = abs(sum(v for v in tracker.components.values() if v < 0))
    except Exception as e:
        logger.warning("Failed to apply dynamic weights: %s", e)

    max_total_penalty = float(os.getenv("SCOUT_MAX_TOTAL_PENALTY", "80.0"))
    penalty_cap_enabled = os.getenv("SCOUT_PENALTY_CAP_ENABLED", "true").lower() == "true"
    penalty_precedence_enabled = os.getenv("SCOUT_PENALTY_PRECEDENCE", "true").lower() == "true"

    # Certain penalty categories indicate fundamentally dangerous behaviour
    # and should never be diluted by a proportional cap.
    UNCAPPABLE_PENALTIES = {PenaltyCategory.SNIPER, PenaltyCategory.SCAM}

    if penalty_cap_enabled and tracker.negative > 0:
        total_negative_before = tracker.negative

        if tracker.negative > max_total_penalty:
            # Separate cappable from uncappable penalties.
            # String-keyed negatives (e.g. "pf_score", "enhanced_momentum")
            # are never uncappable — they are always proportional to the penalty.
            cappable = abs(sum(
                v for k, v in tracker.components.items()
                if v < 0 and not (isinstance(k, PenaltyCategory) and k in UNCAPPABLE_PENALTIES)
            ))
            uncappable = abs(sum(
                v for k, v in tracker.components.items()
                if isinstance(k, PenaltyCategory) and k in UNCAPPABLE_PENALTIES and v < 0
            ))
            if (cappable + uncappable) > max_total_penalty:
                new_cappable_target = max(0.0, max_total_penalty - uncappable)
                if cappable > 0:
                    scale = new_cappable_target / cappable
                    for name in list(tracker.components.keys()):
                        if tracker.components[name] < 0:
                            if isinstance(name, PenaltyCategory) and name in UNCAPPABLE_PENALTIES:
                                continue  # Preserve at full strength
                            tracker.components[name] *= scale
                tracker.negative = abs(sum(
                    v for v in tracker.components.values() if v < 0
                ))

                logger.debug(
                    f"Penalty cap applied: {total_negative_before:.1f} -> {tracker.negative:.1f} "
                    f"(cappable={cappable:.1f}, uncappable={uncappable:.1f})"
                )

        if penalty_precedence_enabled:
            tracker._apply_penalty_precedence()

        tracker._apply_penalty_confidence()

        # Advanced Risk Features Integration (CVaR, Drawdown Duration, Ulcer Index)
        # These provide sophisticated risk detection beyond basic drawdown percentage
        if hasattr(metrics, 'advanced_risk_features') and metrics.advanced_risk_features:
            arf = metrics.advanced_risk_features

            # Only apply if extraction was successful with complete data
            # Require: extraction_success=True, sample_count>=5, all required fields present, no extraction errors
            if (arf.get('extraction_success') and
                arf.get('sample_count', 0) >= 5 and
                all(key in arf for key in ['cvar_95', 'max_drawdown_duration_trades', 'ulcer_index']) and
                not arf.get('extraction_errors')):

                # Apply CVaR penalty (95th percentile conditional value at risk)
                # CVaR measures average loss in the worst 5% of trades
                # Negative CVaR indicates losses - penalize proportional to severity
                cvar_95 = arf.get('cvar_95', 0.0)
                if cvar_95 < 0:  # Only penalize negative CVaR (losses)
                    cvar_penalty = abs(cvar_95) * 0.2
                    tracker.add_neg(PenaltyCategory.CVAR, cvar_penalty)
                    logger.debug(
                        f"CVaR penalty applied: {cvar_penalty:.2f} "
                        f"(cvar_95={cvar_95:.4f})"
                    )

                # Apply Drawdown Duration penalty
                # Long drawdowns indicate slow recovery from losses
                # More than 10 trades to recover suggests poor risk management
                max_dd_duration = arf.get('max_drawdown_duration_trades', 0)
                if max_dd_duration > 10:  # More than 10 trades to recover
                    dd_duration_penalty = max_dd_duration * 0.1
                    tracker.add_neg(PenaltyCategory.DRAWDOWN_DURATION, dd_duration_penalty)
                    logger.debug(
                        f"Drawdown duration penalty applied: {dd_duration_penalty:.2f} "
                        f"(max_drawdown_duration_trades={max_dd_duration})"
                    )

                # Apply Ulcer Index penalty (depth + duration combined metric)
                # Ulcer Index > 5.0 indicates severe, prolonged drawdowns
                ulcer_index = arf.get('ulcer_index', 0.0)
                if ulcer_index > 5.0:  # Severe, prolonged drawdown
                    ulcer_penalty = min(20.0, ulcer_index * 0.5)
                    tracker.add_neg(PenaltyCategory.ULCER_INDEX, ulcer_penalty)
                    logger.debug(
                        f"Ulcer Index penalty applied: {ulcer_penalty:.2f} "
                        f"(ulcer_index={ulcer_index:.2f})"
                    )

    return tracker.to_components()


def calculate_wqs(metrics: WalletMetrics, strategy: str = "SHIELD") -> float:
    # Short-circuit ARBITRAGE wallets (bot behavior, not directional traders)
    if metrics.archetype == "ARBITRAGE":
        return 0.0

    components = _calculate_raw_score(metrics, strategy=strategy)
    if components.is_instant_reject:
        return 0.0

    return components.raw_score


def _compute_confidence(trade_count: int, profit_factor: Optional[float] = None, metrics: Optional[WalletMetrics] = None, is_unproven: bool = False) -> float:
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

    if metrics is not None and metrics.avg_trade_size_sol is not None:
        avg_size = float(metrics.avg_trade_size_sol)
        size_factor = min(1.0, avg_size / 0.5)
        confidence = confidence * (0.5 + 0.5 * size_factor)

    if is_unproven and confidence > 0.70:
        confidence = 0.70

    if metrics is not None and metrics.parse_rate is not None:
        parse_cap = 0.30 + metrics.parse_rate * 0.70
        if confidence > parse_cap:
            confidence = parse_cap

    return max(0.0, min(confidence, 1.0))


def calculate_wqs_with_confidence(metrics: WalletMetrics, strategy: str = "SHIELD") -> WqsResult:
    # Short-circuit ARBITRAGE wallets (bot behavior, not directional traders)
    if metrics.archetype == "ARBITRAGE":
        return WqsResult(score=0.0, confidence=0.0, adjusted_score=0.0)

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
    active_threshold: float = 75.0,
    candidate_threshold: float = 50.0,
    confidence: Optional[float] = None,
    min_confidence: float = 0.70,
) -> str:
    if wqs_score >= active_threshold:
        if confidence is not None and confidence < min_confidence:
            return "CANDIDATE"
        return "ACTIVE"
    elif wqs_score >= candidate_threshold:
        return "CANDIDATE"
    else:
        return "REJECTED"


if __name__ == "__main__":
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
