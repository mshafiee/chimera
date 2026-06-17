"""
Real-Time Profit Tracker for Optimization Feedback

This module implements real-time profit tracking to provide immediate feedback
for optimization decisions and calculate ETA to $1,000 goal.

Features:
- Real-time profit tracking with velocity calculation
- ETA projection to $1,000 target
- ROI tracking by category and WQS band
- Optimization triggering based on performance
- Growth stage detection

Target: Grow from $200 to $1,000 to afford Helius Business Plan upgrade
"""

import os
import time
import logging
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass, field
from enum import Enum
import threading
import json
from pathlib import Path
from collections import deque

logger = logging.getLogger(__name__)


class GrowthStage(Enum):
    """Capital growth stages for optimization targets."""
    EARLY = "early"          # $200-300
    MID = "mid"              # $300-500
    GROWTH = "growth"        # $500-800
    FINAL = "final"          # $800+


class OptimizationTrigger(Enum):
    """Triggers for optimization actions."""
    VELOCITY_LOW = "velocity_low"           # Profit velocity declining
    VELOCITY_HIGH = "velocity_high"         # Profit velocity high (expand)
    WIN_RATE_LOW = "win_rate_low"           # Win rate below threshold
    WIN_RATE_HIGH = "win_rate_high"         # Win rate high (expand)
    DRAWDOWN_EXCEEDED = "drawdown_exceeded" # Drawdown too high
    TARGET_REACHED = "target_reached"       # $1,000 reached!


@dataclass
class ProfitSnapshot:
    """Snapshot of profit at a point in time."""
    timestamp: float
    capital: float
    profit: float
    profit_pct: float
    growth_stage: GrowthStage


@dataclass
class ProfitVelocity:
    """Profit velocity metrics."""
    hourly_rate: float    # $/hour
    daily_rate: float      # $/day
    weekly_rate: float     # $/week
    trend: str             # "increasing", "stable", "decreasing"
    timestamp: float


@dataclass
class ETACalculation:
    """ETA calculation for reaching target."""
    target_capital: float
    current_capital: float
    remaining: float
    days_remaining: float
    hours_remaining: float
    confidence: float      # 0-1 based on velocity stability
    timestamp: float


@dataclass
class OptimizationAction:
    """Suggested optimization action."""
    trigger: OptimizationTrigger
    action: str
    priority: str  # "high", "medium", "low"
    description: str
    expected_impact: str
    timestamp: float = field(default_factory=time.time)


@dataclass
class TrackerConfig:
    """Configuration for profit tracker."""
    STARTING_CAPITAL: float = 200.0
    TARGET_CAPITAL: float = 1000.0

    # Growth stage thresholds
    STAGE_EARLY_MAX: float = 300.0
    STAGE_MID_MAX: float = 500.0
    STAGE_GROWTH_MAX: float = 800.0

    # Velocity calculation windows
    VELOCITY_WINDOW_HOURS: int = 24
    VELOCITY_MIN_SAMPLES: int = 5

    # Thresholds for optimization triggers
    MIN_HOURLY_VELOCITY: float = 0.5      # $0.50/hour minimum
    TARGET_DAILY_VELOCITY: float = 5.0    # $5/day target
    MIN_WIN_RATE: float = 0.50            # 50% minimum
    MAX_DRAWDOWN_PCT: float = 0.30        # 30% max drawdown

    # ETA calculation
    ETA_CONFIDENCE_MIN_SAMPLES: int = 10
    ETA_STABILITY_THRESHOLD: float = 0.3  # Coefficient of variation threshold

    # State persistence
    STATE_FILE: str = "realtime_profit_tracker_state.json"


class RealtimeProfitTracker:
    """
    Real-time profit tracker for optimization feedback.

    Features:
    - Real-time profit tracking
    - Velocity calculation ($/hour, $/day)
    - ETA projection to $1,000
    - Optimization triggering
    - Growth stage detection
    """

    def __init__(self, config: Optional[TrackerConfig] = None):
        """Initialize the profit tracker."""
        self._config = config or TrackerConfig()
        self._lock = threading.Lock()

        # Current capital state
        self._current_capital = self._config.STARTING_CAPITAL
        self._starting_capital = self._config.STARTING_CAPITAL

        # Profit history for velocity calculation
        self._profit_history: deque = deque(maxlen=1000)

        # Trade history for win rate calculation
        self._trade_history: deque = deque(maxlen=500)

        # ROI tracking by category
        self._category_roi: Dict[str, Dict[str, float]] = {}

        # ROI tracking by WQS band
        self._wqs_band_roi: Dict[str, Dict[str, float]] = {
            'very_high': {'profit': 0.0, 'trades': 0},  # WQS 80+
            'high': {'profit': 0.0, 'trades': 0},        # WQS 70-80
            'medium': {'profit': 0.0, 'trades': 0},      # WQS 50-70
            'emerging': {'profit': 0.0, 'trades': 0},    # WQS 30-50
            'low': {'profit': 0.0, 'trades': 0},         # WQS < 30
        }

        # Peak capital for drawdown calculation
        self._peak_capital = self._starting_capital

        # Last velocity calculation
        self._last_velocity: Optional[ProfitVelocity] = None

        # Load state if available
        self._load_state()

        logger.info(f"RealtimeProfitTracker initialized: ${self._current_capital:.2f}")

    def update_profit(self, trade_id: str, pnl: float, wqs: Optional[float] = None,
                     category: Optional[str] = None) -> None:
        """
        Update profit tracking with a trade result.

        Args:
            trade_id: Unique trade identifier
            pnl: Profit/loss amount
            wqs: Optional WQS score of wallet
            category: Optional budget category
        """
        with self._lock:
            now = time.time()

            # Update capital
            self._current_capital += pnl

            # Update peak capital
            if self._current_capital > self._peak_capital:
                self._peak_capital = self._current_capital

            # Record profit snapshot
            profit = self._current_capital - self._starting_capital
            profit_pct = (profit / self._starting_capital) * 100 if self._starting_capital > 0 else 0

            snapshot = ProfitSnapshot(
                timestamp=now,
                capital=self._current_capital,
                profit=profit,
                profit_pct=profit_pct,
                growth_stage=self._get_growth_stage(),
            )
            self._profit_history.append(snapshot)

            # Record trade
            self._trade_history.append({
                'timestamp': now,
                'pnl': pnl,
                'win': pnl > 0,
                'wqs': wqs,
                'category': category,
            })

            # Update category ROI
            if category:
                if category not in self._category_roi:
                    self._category_roi[category] = {'profit': 0.0, 'trades': 0}
                self._category_roi[category]['profit'] += pnl
                self._category_roi[category]['trades'] += 1

            # Update WQS band ROI
            if wqs is not None:
                band = self._get_wqs_band(wqs)
                self._wqs_band_roi[band]['profit'] += pnl
                self._wqs_band_roi[band]['trades'] += 1

            logger.debug(
                f"Updated profit: ${pnl:+.2f} -> Total: ${self._current_capital:.2f} "
                f"({profit_pct:+.1f}%)"
            )

    def _get_wqs_band(self, wqs: float) -> str:
        """Get WQS band for ROI tracking."""
        if wqs >= 80:
            return 'very_high'
        elif wqs >= 70:
            return 'high'
        elif wqs >= 50:
            return 'medium'
        elif wqs >= 30:
            return 'emerging'
        else:
            return 'low'

    def _get_growth_stage(self) -> GrowthStage:
        """Get current growth stage based on capital."""
        capital = self._current_capital

        if capital < self._config.STAGE_EARLY_MAX:
            return GrowthStage.EARLY
        elif capital < self._config.STAGE_MID_MAX:
            return GrowthStage.MID
        elif capital < self._config.STAGE_GROWTH_MAX:
            return GrowthStage.GROWTH
        else:
            return GrowthStage.FINAL

    def get_current_profit(self) -> float:
        """Get current total profit."""
        with self._lock:
            return self._current_capital - self._starting_capital

    def get_current_capital(self) -> float:
        """Get current capital."""
        with self._lock:
            return self._current_capital

    def get_profit_velocity(self) -> ProfitVelocity:
        """
        Calculate current profit velocity.

        Returns:
            ProfitVelocity with rates and trend
        """
        with self._lock:
            if len(self._profit_history) < self._config.VELOCITY_MIN_SAMPLES:
                # Not enough data, return zero velocity
                return ProfitVelocity(
                    hourly_rate=0.0,
                    daily_rate=0.0,
                    weekly_rate=0.0,
                    trend="stable",
                    timestamp=time.time(),
                )

            now = time.time()
            window_start = now - (self._config.VELOCITY_WINDOW_HOURS * 3600)

            # Get snapshots within window
            recent_snapshots = [
                s for s in self._profit_history
                if s.timestamp >= window_start
            ]

            if len(recent_snapshots) < 2:
                # Need at least 2 snapshots for velocity
                return ProfitVelocity(
                    hourly_rate=0.0,
                    daily_rate=0.0,
                    weekly_rate=0.0,
                    trend="stable",
                    timestamp=now,
                )

            # Calculate velocity from first to last snapshot
            first = recent_snapshots[0]
            last = recent_snapshots[-1]

            time_diff_hours = (last.timestamp - first.timestamp) / 3600
            if time_diff_hours == 0:
                time_diff_hours = 1  # Avoid division by zero

            profit_diff = last.profit - first.profit
            hourly_rate = profit_diff / time_diff_hours
            daily_rate = hourly_rate * 24
            weekly_rate = daily_rate * 7

            # Determine trend
            if self._last_velocity:
                if hourly_rate > self._last_velocity.hourly_rate * 1.1:
                    trend = "increasing"
                elif hourly_rate < self._last_velocity.hourly_rate * 0.9:
                    trend = "decreasing"
                else:
                    trend = "stable"
            else:
                trend = "stable"

            velocity = ProfitVelocity(
                hourly_rate=hourly_rate,
                daily_rate=daily_rate,
                weekly_rate=weekly_rate,
                trend=trend,
                timestamp=now,
            )

            self._last_velocity = velocity
            return velocity

    def get_eta_to_1000(self) -> ETACalculation:
        """
        Calculate ETA to reach $1,000 target.

        Returns:
            ETACalculation with time remaining and confidence
        """
        with self._lock:
            velocity = self.get_profit_velocity()
            remaining = self._config.TARGET_CAPITAL - self._current_capital

            # If already at or past target
            if remaining <= 0:
                return ETACalculation(
                    target_capital=self._config.TARGET_CAPITAL,
                    current_capital=self._current_capital,
                    remaining=0,
                    days_remaining=0,
                    hours_remaining=0,
                    confidence=1.0,
                    timestamp=time.time(),
                )

            # Calculate ETA based on daily rate
            if velocity.daily_rate <= 0:
                # Not profitable or no velocity data
                return ETACalculation(
                    target_capital=self._config.TARGET_CAPITAL,
                    current_capital=self._current_capital,
                    remaining=remaining,
                    days_remaining=float('inf'),
                    hours_remaining=float('inf'),
                    confidence=0.0,
                    timestamp=time.time(),
                )

            # Calculate time remaining
            days_remaining = remaining / velocity.daily_rate
            hours_remaining = days_remaining * 24

            # Calculate confidence based on velocity stability
            confidence = self._calculate_eta_confidence()

            return ETACalculation(
                target_capital=self._config.TARGET_CAPITAL,
                current_capital=self._current_capital,
                remaining=remaining,
                days_remaining=days_remaining,
                hours_remaining=hours_remaining,
                confidence=confidence,
                timestamp=time.time(),
            )

    def _calculate_eta_confidence(self) -> float:
        """Calculate confidence in ETA projection."""
        if len(self._profit_history) < self._config.ETA_CONFIDENCE_MIN_SAMPLES:
            return 0.3  # Low confidence with insufficient data

        # Calculate coefficient of variation in recent profits
        recent_snapshots = list(self._profit_history)[-50:]
        if len(recent_snapshots) < 2:
            return 0.5

        profits = [s.profit for s in recent_snapshots]
        if not profits:
            return 0.5

        # Calculate CV
        import statistics
        mean_profit = statistics.mean(profits)
        if mean_profit == 0:
            return 0.5

        try:
            std_profit = statistics.stdev(profits)
            cv = abs(std_profit / mean_profit)

            # Convert CV to confidence (lower CV = higher confidence)
            confidence = max(0.1, min(1.0, 1.0 - (cv / self._config.ETA_STABILITY_THRESHOLD)))
            return confidence
        except statistics.StatisticsError:
            return 0.5

    def trigger_optimization_if_needed(self) -> List[OptimizationAction]:
        """
        Generate optimization actions based on current performance.

        Returns:
            List of suggested optimization actions
        """
        with self._lock:
            actions = []

            # Check if target reached
            if self._current_capital >= self._config.TARGET_CAPITAL:
                actions.append(OptimizationAction(
                    trigger=OptimizationTrigger.TARGET_REACHED,
                    action="upgrade_business_plan",
                    priority="high",
                    description=f"Target ${self._config.TARGET_CAPITAL:.0f} reached! Upgrade to Helius Business Plan.",
                    expected_impact="Unlocks 100M credits, 200 RPS, LaserStream gRPC",
                ))
                return actions

            velocity = self.get_profit_velocity()
            win_rate = self._calculate_win_rate()
            drawdown = self._calculate_drawdown()

            # Check for low velocity
            if velocity.daily_rate < self._config.TARGET_DAILY_VELOCITY * 0.5:
                actions.append(OptimizationAction(
                    trigger=OptimizationTrigger.VELOCITY_LOW,
                    action="tighten_signal_filter",
                    priority="high",
                    description=f"Daily velocity ${velocity.daily_rate:.2f} below 50% of target ${self._config.TARGET_DAILY_VELOCITY:.2f}",
                    expected_impact="Improve signal quality to increase win rate",
                ))

            # Check for low win rate
            if win_rate < self._config.MIN_WIN_RATE and len(self._trade_history) >= 10:
                actions.append(OptimizationAction(
                    trigger=OptimizationTrigger.WIN_RATE_LOW,
                    action="increase_high_conviction_focus",
                    priority="high",
                    description=f"Win rate {win_rate:.1%} below {self._config.MIN_WIN_RATE:.1%} threshold",
                    expected_impact="Focus on WQS 70+ wallets to improve win rate",
                ))

            # Check for excessive drawdown
            if drawdown > self._config.MAX_DRAWDOWN_PCT:
                actions.append(OptimizationAction(
                    trigger=OptimizationTrigger.DRAWDOWN_EXCEEDED,
                    action="reduce_position_sizes",
                    priority="high",
                    description=f"Drawdown {drawdown:.1%} exceeds {self._config.MAX_DRAWDOWN_PCT:.1%} maximum",
                    expected_impact="Reduce risk and preserve capital",
                ))

            # Check for high performance (expansion opportunities)
            if velocity.daily_rate > self._config.TARGET_DAILY_VELOCITY * 1.5:
                actions.append(OptimizationAction(
                    trigger=OptimizationTrigger.VELOCITY_HIGH,
                    action="loosen_signal_filter",
                    priority="medium",
                    description=f"Daily velocity ${velocity.daily_rate:.2f} exceeds 150% of target",
                    expected_impact="Increase signal volume to maximize growth",
                ))

            if win_rate > 0.65 and len(self._trade_history) >= 10:
                actions.append(OptimizationAction(
                    trigger=OptimizationTrigger.WIN_RATE_HIGH,
                    action="expand_analysis_budget",
                    priority="low",
                    description=f"Win rate {win_rate:.1%} excellent, consider expanding",
                    expected_impact="Find more high-quality wallets to scale",
                ))

            return actions

    def _calculate_win_rate(self) -> float:
        """Calculate current win rate."""
        if not self._trade_history:
            return 0.0

        wins = sum(1 for t in self._trade_history if t.get('win', False))
        return wins / len(self._trade_history)

    def _calculate_drawdown(self) -> float:
        """Calculate current drawdown percentage."""
        if self._peak_capital == 0:
            return 0.0

        return (self._peak_capital - self._current_capital) / self._peak_capital

    def get_category_roi(self, category: str) -> Dict[str, float]:
        """Get ROI metrics for a category."""
        with self._lock:
            if category not in self._category_roi:
                return {'profit': 0.0, 'trades': 0, 'avg_pnl': 0.0, 'roi': 0.0}

            data = self._category_roi[category]
            avg_pnl = data['profit'] / max(1, data['trades'])

            return {
                'profit': data['profit'],
                'trades': data['trades'],
                'avg_pnl': avg_pnl,
                'roi': avg_pnl / max(0.01, self._starting_capital),
            }

    def get_wqs_band_roi(self, band: str) -> Dict[str, float]:
        """Get ROI metrics for a WQS band."""
        with self._lock:
            if band not in self._wqs_band_roi:
                return {'profit': 0.0, 'trades': 0, 'avg_pnl': 0.0, 'win_rate': 0.0}

            data = self._wqs_band_roi[band]
            avg_pnl = data['profit'] / max(1, data['trades'])

            # Calculate win rate from trade history
            band_trades = [t for t in self._trade_history if self._get_wqs_band(t.get('wqs', 0)) == band]
            wins = sum(1 for t in band_trades if t.get('win', False)) if band_trades else 0
            win_rate = wins / len(band_trades) if band_trades else 0.0

            return {
                'profit': data['profit'],
                'trades': data['trades'],
                'avg_pnl': avg_pnl,
                'win_rate': win_rate,
            }

    def get_tracker_summary(self) -> Dict[str, Any]:
        """Get comprehensive tracker summary."""
        with self._lock:
            velocity = self.get_profit_velocity()
            eta = self.get_eta_to_1000()

            return {
                'capital': {
                    'starting': self._starting_capital,
                    'current': self._current_capital,
                    'profit': self.get_current_profit(),
                    'profit_pct': ((self._current_capital - self._starting_capital) /
                                  max(0.01, self._starting_capital)) * 100,
                    'peak': self._peak_capital,
                    'drawdown_pct': self._calculate_drawdown() * 100,
                    'growth_stage': self._get_growth_stage().value,
                },
                'velocity': {
                    'hourly': velocity.hourly_rate,
                    'daily': velocity.daily_rate,
                    'weekly': velocity.weekly_rate,
                    'trend': velocity.trend,
                },
                'eta': {
                    'target': self._config.TARGET_CAPITAL,
                    'remaining': eta.remaining,
                    'days_remaining': eta.days_remaining,
                    'hours_remaining': eta.hours_remaining,
                    'confidence': eta.confidence,
                },
                'performance': {
                    'win_rate': self._calculate_win_rate(),
                    'total_trades': len(self._trade_history),
                    'category_roi': {cat: self.get_category_roi(cat) for cat in self._category_roi},
                    'wqs_band_roi': {band: self.get_wqs_band_roi(band) for band in self._wqs_band_roi},
                },
            }

    def _load_state(self) -> None:
        """Load state from disk."""
        state_file = Path(self._config.STATE_FILE)
        if not state_file.exists():
            return

        try:
            with open(state_file, 'r') as f:
                data = json.load(f)

            self._current_capital = data.get('current_capital', self._config.STARTING_CAPITAL)
            self._peak_capital = data.get('peak_capital', self._config.STARTING_CAPITAL)
            self._category_roi = data.get('category_roi', {})

            logger.info(f"Loaded state from {state_file}")

        except Exception as e:
            logger.warning(f"Failed to load state: {e}")

    def save_state(self) -> None:
        """Save state to disk."""
        with self._lock:
            try:
                data = {
                    'current_capital': self._current_capital,
                    'peak_capital': self._peak_capital,
                    'category_roi': self._category_roi,
                    'last_save': time.time(),
                }

                state_file = Path(self._config.STATE_FILE)
                with open(state_file, 'w') as f:
                    json.dump(data, f, indent=2)

            except Exception as e:
                logger.warning(f"Failed to save state: {e}")

    def reset_to_capital(self, new_capital: float) -> None:
        """Reset tracker to a new capital amount."""
        with self._lock:
            self._current_capital = new_capital
            self._starting_capital = new_capital
            self._peak_capital = new_capital
            self._profit_history.clear()
            self._trade_history.clear()
            logger.info(f"Reset tracker to capital: ${new_capital:.2f}")
