"""
Position Management Interface for Stop-Loss Integration

Bridges Scout's stop-loss optimization with Operator's position management.
Provides a unified interface for dynamic stop-loss calculation and management.
"""

import logging
from typing import Dict, List, Optional, Any
from dataclasses import dataclass, field
from enum import Enum
import time
import threading

logger = logging.getLogger(__name__)


class PositionStatus(Enum):
    """Position status types."""
    PENDING = "pending"          # Not yet entered
    ACTIVE = "active"          # Currently open
    EXITING = "exiting"        # Stop-loss triggered
    CLOSED = "closed"          # Position closed
    FAILED = "failed"          # Entry/exit failed


class PositionSide(Enum):
    """Position side (long/short)."""
    LONG = "long"
    SHORT = "short"


@dataclass
class Position:
    """
    Trading position with stop-loss data.

    Represents a trading position with comprehensive stop-loss information
    for dynamic risk management.
    """
    # Basic position info
    position_id: str
    wallet_address: str
    token_address: str
    token_symbol: str
    side: PositionSide

    # Entry and current state
    entry_price: float
    current_price: float
    position_size_sol: float
    position_value_usd: float

    # Stop-loss data
    stop_loss_price: Optional[float] = None
    stop_type: str = "ATR"  # ATR, FIXED, TRAILING
    regime: str = "NEUTRAL"
    atr_value: Optional[float] = None
    multiplier_used: float = 1.0

    # Tracking
    status: PositionStatus = PositionStatus.PENDING
    unrealized_pnl: float = 0.0
    realized_pnl: float = 0.0
    max_profit: float = 0.0  # Highest profit reached (for trailing stops)
    max_drawdown: float = 0.0  # Deepest drawdown experienced

    # Timestamps
    created_at: float = field(default_factory=time.time)
    updated_at: float = field(default_factory=time.time)
    stop_triggered_at: Optional[float] = None
    closed_at: Optional[float] = None

    # Metadata
    notes: str = ""
    strategy: str = "SHIELD"  # SHIELD or SPEAR
    wqs_score: Optional[float] = None


class PositionManager:
    """
    Position management with stop-loss optimization.

    Integrates with:
    - StopLossOptimizer for stop calculation
    - MarketRegimeDetector for regime-aware stops
    - Operator (Rust) for position execution
    """

    def __init__(self, stop_loss_optimizer, regime_detector=None):
        """
        Initialize position manager.

        Args:
            stop_loss_optimizer: StopLossOptimizer instance
            regime_detector: Optional MarketRegimeDetector instance
        """
        self._stop_loss_optimizer = stop_loss_optimizer
        self._regime_detector = regime_detector
        self._positions: Dict[str, Position] = {}  # position_id -> Position
        self._wallet_positions: Dict[str, List[str]] = {}  # wallet_address -> [position_ids]
        self._lock = threading.Lock()

        logger.info("Position Manager initialized")
        logger.info(f"  Stop-loss optimizer: {type(stop_loss_optimizer).__name__}")
        logger.info(f"  Regime detector: {type(regime_detector).__name__ if regime_detector else 'None'}")

    def create_position(
        self,
        position_id: str,
        wallet_address: str,
        token_address: str,
        token_symbol: str,
        entry_price: float,
        position_size_sol: float,
        position_value_usd: float,
        side: PositionSide = PositionSide.LONG,
        strategy: str = "SHIELD",
        wqs_score: Optional[float] = None,
    ) -> Position:
        """
        Create a new position with initial stop-loss calculation.

        Args:
            position_id: Unique position identifier
            wallet_address: Wallet address that triggered this position
            token_address: Token mint address
            token_symbol: Token symbol
            entry_price: Entry price in USD
            position_size_sol: Position size in SOL
            position_value_usd: Position value in USD
            side: Position side (LONG/SHORT)
            strategy: Trading strategy (SHIELD/SPEAR)
            wqs_score: WQS score of the wallet (optional)

        Returns:
            Created Position with stop-loss calculated
        """
        with self._lock:
            # Create position object
            position = Position(
                position_id=position_id,
                wallet_address=wallet_address,
                token_address=token_address,
                token_symbol=token_symbol,
                side=side,
                entry_price=entry_price,
                current_price=entry_price,
                position_size_sol=position_size_sol,
                position_value_usd=position_value_usd,
                strategy=strategy,
                wqs_score=wqs_score,
                status=PositionStatus.ACTIVE,
            )

            # Calculate initial stop-loss
            position = self.calculate_stop_for_position(position)

            # Store position
            self._positions[position_id] = position

            # Update wallet positions index
            if wallet_address not in self._wallet_positions:
                self._wallet_positions[wallet_address] = []
            self._wallet_positions[wallet_address].append(position_id)

            logger.info(f"Created position {position_id} with stop-loss {position.stop_loss_price}")

            return position

    def calculate_stop_for_position(self, position: Position) -> Position:
        """
        Calculate optimal stop-loss for a position.

        Args:
            position: Position object to calculate stop for

        Returns:
            Position with updated stop-loss data
        """
        try:
            # Get current market regime
            regime = self._regime_detector._current_regime if self._regime_detector else "NEUTRAL"

            # Calculate ATR from wallet performance data (fallback to default if method unavailable)
            atr = 3.0  # Default ATR fallback
            if hasattr(self._stop_loss_optimizer, 'calculate_atr_from_wallet_history'):
                atr = self._stop_loss_optimizer.calculate_atr_from_wallet_history(position.wallet_address)
            elif hasattr(position, 'atr_value') and position.atr_value:
                atr = position.atr_value

            # Calculate stop loss using optimizer
            stop_order = self._stop_loss_optimizer.calculate_atr_stop(
                entry_price=position.entry_price,
                atr_value=atr,
                regime=regime,
                growth_stage="mid"  # Could be dynamic based on token age
            )

            # Update position with stop-loss data
            position.stop_loss_price = stop_order.stop_price
            position.stop_type = stop_order.stop_type.value
            position.regime = regime.value if hasattr(regime, 'value') else regime
            position.atr_value = atr
            position.multiplier_used = stop_order.multiplier_used
            position.updated_at = time.time()

            logger.debug(f"Calculated stop-loss for {position.position_id}: ${position.stop_loss_price:.4f}")

            return position

        except Exception as e:
            logger.error(f"Failed to calculate stop-loss for position {position.position_id}: {e}")
            # Fallback to fixed percentage stop
            position.stop_loss_price = position.entry_price * 0.95  # 5% stop
            position.stop_type = "FIXED"
            position.updated_at = time.time()
            return position

    def update_position_price(self, position_id: str, current_price: float) -> Optional[Position]:
        """
        Update current price for a position and check for stop-loss triggers.

        Args:
            position_id: Position identifier
            current_price: Current token price

        Returns:
            Updated position if found and updated, None otherwise
        """
        with self._lock:
            position = self._positions.get(position_id)
            if not position:
                logger.warning(f"Position {position_id} not found for price update")
                return None

            # Update price and calculate PnL
            old_price = position.current_price
            position.current_price = current_price
            position.updated_at = time.time()

            # Calculate unrealized PnL
            if position.side == PositionSide.LONG:
                pnl_pct = (current_price - position.entry_price) / position.entry_price
            else:
                pnl_pct = (position.entry_price - current_price) / position.entry_price

            position.unrealized_pnl = position.position_value_usd * pnl_pct

            # Track max profit for trailing stops
            if position.unrealized_pnl > position.max_profit:
                position.max_profit = position.unrealized_pnl

            # Track max drawdown
            if position.unrealized_pnl < position.max_drawdown:
                position.max_drawdown = position.unrealized_pnl

            # Check if stop-loss is triggered
            if self._check_stop_loss_triggered(position):
                position.status = PositionStatus.EXITING
                position.stop_triggered_at = time.time()
                logger.info(f"Stop-loss triggered for position {position_id}")

            return position

    def update_trailing_stop(self, position_id: str, current_price: float) -> Optional[Position]:
        """
        Update trailing stop for a position.

        Args:
            position_id: Position identifier
            current_price: Current token price

        Returns:
            Updated position with new trailing stop, None if not found
        """
        with self._lock:
            position = self._positions.get(position_id)
            if not position:
                return None

            # Only update trailing stop if position is profitable
            if position.unrealized_pnl <= 0:
                return position

            try:
                # Get current market regime
                regime = self._regime_detector._current_regime if self._regime_detector else "NEUTRAL"

                # Calculate trailing stop
                trailing_stop = self._stop_loss_optimizer.calculate_trailing_stop(
                    entry_price=position.entry_price,
                    current_price=current_price,
                    atr_value=position.atr_value,
                    regime=regime
                )

                # Update stop-loss to trailing level (only if higher)
                if trailing_stop.stop_price > position.stop_loss_price:
                    position.stop_loss_price = trailing_stop.stop_price
                    position.stop_type = "TRAILING"
                    position.updated_at = time.time()
                    logger.info(f"Updated trailing stop for {position_id} to ${position.stop_loss_price:.4f}")

                return position

            except Exception as e:
                logger.error(f"Failed to update trailing stop for position {position_id}: {e}")
                return position

    def _check_stop_loss_triggered(self, position: Position) -> bool:
        """Check if stop-loss should be triggered."""
        if position.stop_loss_price is None:
            return False

        if position.side == PositionSide.LONG:
            return position.current_price <= position.stop_loss_price
        else:  # SHORT
            return position.current_price >= position.stop_loss_price

    def get_position(self, position_id: str) -> Optional[Position]:
        """Get position by ID."""
        return self._positions.get(position_id)

    def get_wallet_positions(self, wallet_address: str) -> List[Position]:
        """Get all positions for a wallet."""
        position_ids = self._wallet_positions.get(wallet_address, [])
        return [self._positions[pid] for pid in position_ids if pid in self._positions]

    def get_all_positions(self) -> List[Position]:
        """Get all positions."""
        return list(self._positions.values())

    def close_position(self, position_id: str, exit_price: float, exit_reason: str = "") -> Optional[Position]:
        """
        Close a position.

        Args:
            position_id: Position identifier
            exit_price: Exit price
            exit_reason: Reason for closing position

        Returns:
            Closed position or None if not found
        """
        with self._lock:
            position = self._positions.get(position_id)
            if not position:
                logger.warning(f"Position {position_id} not found for closing")
                return None

            # Update position with exit data
            position.current_price = exit_price
            position.status = PositionStatus.CLOSED
            position.closed_at = time.time()

            # Calculate final PnL
            if position.side == PositionSide.LONG:
                pnl_pct = (exit_price - position.entry_price) / position.entry_price
            else:
                pnl_pct = (position.entry_price - exit_price) / position.entry_price

            position.realized_pnl = position.position_value_usd * pnl_pct
            position.notes = exit_reason

            logger.info(f"Closed position {position_id} with PnL: ${position.realized_pnl:.2f}")

            return position

    def get_positions_needing_update(self) -> List[Position]:
        """Get positions that need stop-loss updates (trailing stops, regime changes)."""
        return [
            pos for pos in self._positions.values()
            if pos.status == PositionStatus.ACTIVE and pos.unrealized_pnl > 0
        ]

    def get_summary(self) -> Dict[str, Any]:
        """Get summary of all positions."""
        positions = list(self._positions.values())

        total_value = sum(p.position_value_usd for p in positions)
        total_unrealized_pnl = sum(p.unrealized_pnl for p in positions)
        total_realized_pnl = sum(p.realized_pnl for p in positions if p.status == PositionStatus.CLOSED)

        return {
            "total_positions": len(positions),
            "active_positions": len([p for p in positions if p.status == PositionStatus.ACTIVE]),
            "closed_positions": len([p for p in positions if p.status == PositionStatus.CLOSED]),
            "total_value_usd": total_value,
            "total_unrealized_pnl": total_unrealized_pnl,
            "total_realized_pnl": total_realized_pnl,
            "positions_needing_update": len(self.get_positions_needing_update()),
        }


# Singleton instance
_position_manager: Optional[PositionManager] = None
_manager_lock = threading.Lock()


def get_position_manager() -> Optional[PositionManager]:
    """Get the global position manager singleton."""
    global _position_manager
    with _manager_lock:
        return _position_manager


def set_position_manager(manager: PositionManager):
    """Set the global position manager singleton."""
    global _position_manager
    with _manager_lock:
        _position_manager = manager