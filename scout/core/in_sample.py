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

    # Trade sizes (in-sample only) for archetype adjustments
    trade_sizes = [float(t.amount_sol) for t in in_sample_trades]

    # Last trade timestamp from in-sample
    last_trade = in_sample_trades[-1].timestamp.isoformat()

    # ROI 7d from the LAST 7 DAYS OF THE IN-SAMPLE PERIOD. Anchoring to
    # utcnow() is wrong: the in-sample window typically ended in the past, so
    # the window would be empty (or misaligned) and roi_7d forced to 0.
    cutoff_7d = in_sample_trades[-1].timestamp - timedelta(days=7)
    in_sample_7d = [t for t in in_sample_trades if t.timestamp >= cutoff_7d]
    roi_7d = (
        analyzer._calculate_roi_from_trades(in_sample_7d)
        if in_sample_7d else 0.0
    )

    # Win streak consistency from in-sample
    win_streak = analyzer._calculate_win_streak_consistency(in_sample_trades)

    # Risk/behavior metrics recomputed from the in-sample window. The
    # full-history versions include the holdout (newest 30%) trades, which
    # would reintroduce look-ahead bias into the WQS score.
    _, _, _, per_trade_pnl, _ = analyzer._replay_positions(in_sample_trades)
    pnl_list = [float(v) for v in per_trade_pnl.values()]
    realized_profit = sum(max(0.0, p) for p in pnl_list)
    if len(pnl_list) >= 2:
        mean_pnl = sum(pnl_list) / len(pnl_list)
        volatility_30d = (sum((p - mean_pnl) ** 2 for p in pnl_list) / len(pnl_list)) ** 0.5
        downsides = [p for p in pnl_list if p < 0]
        downside_dev = (sum(p * p for p in downsides) / len(downsides)) ** 0.5 if downsides else 0.0
        sortino_ratio = (mean_pnl / downside_dev) if downside_dev > 0 else 0.0
    else:
        volatility_30d = 0.0
        sortino_ratio = 0.0

    # Avg hold time from in-sample trades (seconds -> hours)
    hold_seconds = analyzer._calculate_avg_hold_time(in_sample_trades)
    avg_hold_time_hours = (hold_seconds / 3600.0) if hold_seconds is not None else None

    return WalletMetrics(
        address=full_metrics.address,
        # Recalculated from in-sample
        roi_7d=roi_7d,
        roi_30d=roi,  # in-sample spans ~21d — best available proxy for 30d
        roi_90d=roi,  # same window; avoids full-history look-ahead
        trade_count_30d=trade_count,
        win_rate=win_rate,
        max_drawdown_30d=max_drawdown,
        avg_trade_size_sol=avg_size,
        last_trade_at=last_trade,
        profit_factor=profit_factor,
        win_streak_consistency=win_streak,
        sortino_ratio=sortino_ratio,
        volatility_30d=volatility_30d,
        trade_sizes=trade_sizes,
        avg_hold_time_hours=avg_hold_time_hours,
        total_realized_profit_sol=realized_profit,
        # Unrealized amounts excluded (marking open positions to market would
        # use current prices — pure look-ahead in a historical window)
        total_unrealized_loss_sol=None,
        total_unrealized_gain_sol=None,
        # Entry delay requires async token-creation lookups; excluded rather
        # than leaking the full-history value
        avg_entry_delay_seconds=None,
        # Carried from full metrics (structural, no future leakage)
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
    )
