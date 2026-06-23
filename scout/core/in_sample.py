"""
In-sample metric computation for clean WQS scoring.

Splits trades chronologically and recomputes financial metrics from the
older portion, preventing look-ahead bias in Wallet Quality Score calculation.
"""

from datetime import timedelta
from decimal import Decimal
from typing import Optional

from .analyzer import WalletAnalyzer
from .models import TradeAction
from .utils import utcnow
from .wqs import WalletMetrics


def compute_in_sample_metrics(
    analyzer: WalletAnalyzer,
    in_sample_trades: list,
    full_metrics: WalletMetrics,
) -> Optional[WalletMetrics]:
    """Compute WQS-critical financial metrics from in-sample trades only.

    Financial fields (roi, win_rate, drawdown, profit_factor, trade_count)
    are recomputed from the in-sample period. Structural wallet properties
    (DEX diversity, MEV protection, limit orders, scam correlation, etc.)
    are carried over from full_metrics since they don't leak future info.

    Returns None if in_sample_trades is insufficient.
    """
    if not in_sample_trades:
        return None

    # Financial metrics from in-sample only (all synchronous)
    roi = analyzer._calculate_roi_from_trades(in_sample_trades)
    win_rate = analyzer._calculate_win_rate_from_trades(in_sample_trades)
    max_drawdown = analyzer._calculate_drawdown_from_trades(in_sample_trades)

    # Profit factor from in-sample realized closes
    closes = [t for t in in_sample_trades
              if t.action == TradeAction.SELL and t.pnl_sol is not None]
    gross_profit = sum(t.pnl_sol for t in closes if t.pnl_sol > Decimal('0'))
    gross_loss = abs(sum(t.pnl_sol for t in closes if t.pnl_sol < Decimal('0')))
    win_count = sum(1 for t in closes if t.pnl_sol > Decimal('0'))
    profit_factor = analyzer._compute_base_profit_factor(
        gross_profit, gross_loss, win_count
    )
    trade_count = len(closes)

    # Average trade size from in-sample (all trades, not just closes)
    avg_size = float(
        sum(t.amount_sol for t in in_sample_trades)
        / max(1, len(in_sample_trades))
    )

    # Last trade timestamp from in-sample
    last_trade = (
        in_sample_trades[-1].timestamp.isoformat()
        if in_sample_trades else None
    )

    # ROI 7d from last 7 days of in-sample period
    cutoff_7d = utcnow() - timedelta(days=7)
    in_sample_7d = [t for t in in_sample_trades if t.timestamp >= cutoff_7d]
    roi_7d = (
        analyzer._calculate_roi_from_trades(in_sample_7d)
        if in_sample_7d else 0.0
    )

    # Win streak consistency from in-sample
    win_streak = analyzer._calculate_win_streak_consistency(in_sample_trades)

    return WalletMetrics(
        address=full_metrics.address,
        # Recalculated from in-sample
        roi_7d=roi_7d,
        roi_30d=roi,  # in-sample spans ~21d — best available proxy for 30d
        trade_count_30d=trade_count,
        win_rate=win_rate,
        max_drawdown_30d=max_drawdown,
        avg_trade_size_sol=avg_size,
        last_trade_at=last_trade,
        profit_factor=profit_factor,
        win_streak_consistency=win_streak,
        # Carried from full metrics (structural, no future leakage)
        roi_90d=full_metrics.roi_90d,
        is_fresh_wallet=full_metrics.is_fresh_wallet,
        is_unproven=full_metrics.is_unproven,
        parse_rate=full_metrics.parse_rate,
        uses_limit_orders=full_metrics.uses_limit_orders,
        uses_mev_protection=full_metrics.uses_mev_protection,
        correlated_with_scam=full_metrics.correlated_with_scam,
        mev_risk_score=full_metrics.mev_risk_score,
        dex_diversity_score=full_metrics.dex_diversity_score,
        unique_token_categories=full_metrics.unique_token_categories,
        archetype=full_metrics.archetype,
        trajectory=full_metrics.trajectory,
        sortino_ratio=full_metrics.sortino_ratio,
        avg_entry_delay_seconds=full_metrics.avg_entry_delay_seconds,
        total_unrealized_loss_sol=full_metrics.total_unrealized_loss_sol,
        total_realized_profit_sol=full_metrics.total_realized_profit_sol,
        total_unrealized_gain_sol=full_metrics.total_unrealized_gain_sol,
        volatility_30d=full_metrics.volatility_30d,
        trade_sizes=full_metrics.trade_sizes,
        avg_hold_time_hours=full_metrics.avg_hold_time_hours,
    )
