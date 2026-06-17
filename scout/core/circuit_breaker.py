"""
Multi-Level Circuit Breaker for Trading Operations

This module implements a sophisticated circuit breaker system with multiple protection levels:
- Level 1 (Warning): 5% daily drawdown → Reduce Spear allocation 50%
- Level 2 (Caution): 10% daily drawdown → Halt Spear trading
- Level 3 (Emergency): 15% daily drawdown → Halt all trading

Features:
- Dynamic threshold adjustment based on market volatility
- Per-wallet circuit breaking
- Recovery cooldown periods
- Progressive alarm escalation
- Capital preservation focus
- Integration with production monitoring
"""

import os
import time
import logging
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple, Callable, Any
from dataclasses import dataclass, field
from enum import Enum
import threading
import json
from pathlib import Path

logger = logging.getLogger(__name__)


class CircuitState(Enum):
    """Circuit breaker states."""
    CLOSED = "closed"          # Normal operation
    OPEN = "open"              # Trading halted
    HALF_OPEN = "half_open"    # Testing recovery


class ProtectionLevel(Enum):
    """Protection levels for drawdown thresholds."""
    NORMAL = 1      # Normal operation
    WARNING = 2     # 5% drawdown - Reduce Spear 50%
    CAUTION = 3     # 10% drawdown - Halt Spear
    EMERGENCY = 4   # 15% drawdown - Halt all trading


@dataclass
class CircuitBreakerConfig:
    """Configuration for circuit breaker thresholds."""

    # Drawdown thresholds (percentage of starting capital)
    WARNING_THRESHOLD: float = 0.05    # 5%
    CAUTION_THRESHOLD: float = 0.10    # 10%
    EMERGENCY_THRESHOLD: float = 0.15  # 15%

    # Recovery settings
    RESET_TIMEOUT_SECONDS: int = 300    # 5 minutes before attempting reset
    COOLDOWN_SECONDS: int = 60         # 1 minute cooldown between triggers

    # Daily tracking
    DAILY_START_TIME: float = field(default_factory=time.time)

    # Wallet-specific settings
    MAX_WALLET_FAILURES: int = 5      # Max consecutive failures per wallet
    WALLET_BLACKLIST_DURATION: int = 3600  # 1 hour blacklist

    # Strategy-specific settings
    SHIELD_REDUCTION_FACTOR: float = 0.5  # Reduce Shield by 50% at WARNING
    SPEAR_REDUCTION_FACTOR: float = 1.0   # Halt Spear completely at CAUTION

    # Capital tracking
    STARTING_CAPITAL: float = field(default_factory=lambda: float(os.getenv("SCOUT_STARTING_CAPITAL", "200.0")))
    CURRENT_CAPITAL: float = STARTING_CAPITAL
    PEAK_CAPITAL: float = STARTING_CAPITAL

    # Aggressive mode settings (from user selection)
    AGGRESSIVE_MODE: bool = field(default_factory=lambda: os.getenv("SCOUT_AGGRESSIVE_MODE", "true").lower() == "true")

    # Volatility adjustment
    VOLATILITY_MULTIPLIER: bool = True  # Adjust thresholds based on market volatility


@dataclass
class CircuitBreakerState:
    """Current state of the circuit breaker."""

    current_state: CircuitState = CircuitState.CLOSED
    current_level: ProtectionLevel = ProtectionLevel.NORMAL
    triggered_at: Optional[float] = None
    last_state_change: float = field(default_factory=time.time)
    trigger_count: int = 0
    wallet_blacklist: Dict[str, float] = field(default_factory=dict)  # wallet -> blacklist_until
    daily_trades: int = 0
    daily_success: int = 0
    daily_failures: int = 0
    shield_allocation: float = 0.60  # From aggressive strategy
    spear_allocation: float = 0.40   # From aggressive strategy
    last_adjustment_time: float = field(default_factory=time.time)


@dataclass
class CircuitBreakerEvent:
    """Event recorded by circuit breaker."""

    timestamp: float
    event_type: str  # "trigger", "reset", "adjust", "wallet_blacklist"
    level: ProtectionLevel
    reason: str
    capital: float
    drawdown: float
    metadata: Dict[str, Any] = field(default_factory=dict)


class CircuitBreaker:
    """
    Multi-level circuit breaker for trading protection.

    Implements progressive protection:
    1. WARNING (5% drawdown): Reduce Spear allocation 50%
    2. CAUTION (10% drawdown): Halt Spear trading completely
    3. EMERGENCY (15% drawdown): Halt all trading

    Features:
    - Dynamic threshold adjustment based on volatility
    - Per-wallet circuit breaking
    - Recovery cooldown periods
    - Capital preservation focus
    """

    def __init__(self, config: Optional[CircuitBreakerConfig] = None):
        """Initialize the circuit breaker."""
        self._config = config or CircuitBreakerConfig()
        self._state = CircuitBreakerState()
        self._events: List[CircuitBreakerEvent] = []
        self._lock = threading.Lock()

        # Volatility tracking for dynamic thresholds
        self._volatility_samples: List[float] = []
        self._current_volatility_multiplier = 1.0

        # Load previous state if available
        self._load_state()

        logger.info(f"Circuit Breaker initialized")
        logger.info(f"  Aggressive mode: {self._config.AGGRESSIVE_MODE}")
        logger.info(f"  Starting capital: ${self._config.STARTING_CAPITAL:.0f}")
        logger.info(f"  Current capital: ${self._config.CURRENT_CAPITAL:.0f}")
        logger.info(f"  Current level: {self._state.current_level.name}")

    def _load_state(self):
        """Load previous circuit breaker state from disk."""
        try:
            state_file = Path(os.getenv("SCOUT_CIRCUIT_BREAKER_STATE",
                                       "/tmp/circuit_breaker_state.json"))
            if state_file.exists():
                with open(state_file, 'r') as f:
                    data = json.load(f)

                # Check if state is from today
                state_time = data.get('last_state_change', 0)
                state_datetime = datetime.fromtimestamp(state_time)
                if state_datetime.date() == datetime.now().date():
                    self._state.current_level = ProtectionLevel(
                        data.get('current_level', ProtectionLevel.NORMAL.value)
                    )
                    self._state.triggered_at = data.get('triggered_at')
                    self._state.shield_allocation = data.get('shield_allocation', 0.60)
                    self._state.spear_allocation = data.get('spear_allocation', 0.40)
                    self._config.CURRENT_CAPITAL = data.get('current_capital', self._config.STARTING_CAPITAL)
                    self._config.PEAK_CAPITAL = data.get('peak_capital', self._config.STARTING_CAPITAL)

                    logger.info(f"Loaded previous state: {self._state.current_level.name}")
        except Exception as e:
            logger.warning(f"Failed to load circuit breaker state: {e}")

    def _save_state(self):
        """Save current circuit breaker state to disk."""
        try:
            state_file = Path(os.getenv("SCOUT_CIRCUIT_BREAKER_STATE",
                                       "/tmp/circuit_breaker_state.json"))
            state_file.parent.mkdir(parents=True, exist_ok=True)

            data = {
                'current_level': self._state.current_level.value,
                'triggered_at': self._state.triggered_at,
                'last_state_change': self._state.last_state_change,
                'shield_allocation': self._state.shield_allocation,
                'spear_allocation': self._state.spear_allocation,
                'current_capital': self._config.CURRENT_CAPITAL,
                'peak_capital': self._config.PEAK_CAPITAL,
            }

            with open(state_file, 'w') as f:
                json.dump(data, f, indent=2)
        except Exception as e:
            logger.warning(f"Failed to save circuit breaker state: {e}")

    def get_volatility_multiplier(self) -> float:
        """Calculate volatility multiplier for threshold adjustment."""
        if not self._volatility_samples or len(self._volatility_samples) < 10:
            return 1.0

        avg_volatility = sum(self._volatility_samples[-10:]) / 10
        baseline = 0.02  # 2% daily volatility is normal

        if avg_volatility > baseline * 2:
            # High volatility - widen thresholds by 50%
            return 1.5
        elif avg_volatility > baseline * 1.5:
            return 1.25
        else:
            return 1.0

    def get_drawdown(self) -> float:
        """Calculate current drawdown from peak."""
        if self._config.PEAK_CAPITAL <= 0:
            return 0.0

        return (self._config.PEAK_CAPITAL - self._config.CURRENT_CAPITAL) / self._config.PEAK_CAPITAL

    def check_circuit_breaker(self) -> Tuple[bool, ProtectionLevel, str]:
        """
        Check if circuit breaker should trigger.

        Returns:
            Tuple of (should_trade, current_level, reason)
        """
        with self._lock:
            drawdown = self.get_drawdown()
            volatility_multiplier = self.get_volatility_multiplier() if self._config.VOLATILITY_MULTIPLIER else 1.0

            # Calculate dynamic thresholds
            warning_threshold = self._config.WARNING_THRESHOLD * volatility_multiplier
            caution_threshold = self._config.CAUTION_THRESHOLD * volatility_multiplier
            emergency_threshold = self._config.EMERGENCY_THRESHOLD * volatility_multiplier

            # Determine current protection level
            if drawdown >= emergency_threshold:
                self._trigger(ProtectionLevel.EMERGENCY, f"Drawdown {drawdown*100:.1f}% >= {emergency_threshold*100:.1f}%")
                return False, ProtectionLevel.EMERGENCY, "EMERGENCY: All trading halted"
            elif drawdown >= caution_threshold:
                if self._state.current_level != ProtectionLevel.CAUTION:
                    self._trigger(ProtectionLevel.CAUTION, f"Drawdown {drawdown*100:.1f}% >= {caution_threshold*100:.1f}%")
                return False, ProtectionLevel.CAUTION, "CAUTION: Spear trading halted"
            elif drawdown >= warning_threshold:
                if self._state.current_level != ProtectionLevel.WARNING:
                    self._trigger(ProtectionLevel.WARNING, f"Drawdown {drawdown*100:.1f}% >= {warning_threshold*100:.1f}%")
                return True, ProtectionLevel.WARNING, "WARNING: Spear allocation reduced"
            else:
                # Check if we can reset from previous level
                if self._state.current_level != ProtectionLevel.NORMAL:
                    self._attempt_reset()
                return True, ProtectionLevel.NORMAL, "Normal operation"

    def _trigger(self, level: ProtectionLevel, reason: str):
        """Trigger circuit breaker at specified level."""
        old_level = self._state.current_level
        self._state.current_level = level
        self._state.triggered_at = time.time()
        self._state.last_state_change = time.time()
        self._state.trigger_count += 1

        # Record event
        event = CircuitBreakerEvent(
            timestamp=time.time(),
            event_type="trigger",
            level=level,
            reason=reason,
            capital=self._config.CURRENT_CAPITAL,
            drawdown=self.get_drawdown(),
        )
        self._events.append(event)

        # Apply level-specific adjustments
        self._apply_level_adjustments(level)

        logger.warning(f"Circuit Breaker TRIGGERED: {level.name} - {reason}")
        logger.warning(f"  Capital: ${self._config.CURRENT_CAPITAL:.2f} / ${self._config.PEAK_CAPITAL:.2f}")
        logger.warning(f"  Drawdown: {self.get_drawdown()*100:.1f}%")
        logger.warning(f"  Shield: {self._state.shield_allocation*100:.0f}%, Spear: {self._state.spear_allocation*100:.0f}%")

        self._save_state()

    def _attempt_reset(self):
        """Attempt to reset circuit breaker to normal level."""
        if self._state.triggered_at is None:
            return

        time_since_trigger = time.time() - self._state.triggered_at
        if time_since_trigger >= self._config.RESET_TIMEOUT_SECONDS:
            old_level = self._state.current_level
            self._state.current_level = ProtectionLevel.NORMAL
            self._state.current_state = CircuitState.CLOSED
            self._state.triggered_at = None
            self._state.last_state_change = time.time()

            # Reset allocations to aggressive defaults
            self._state.shield_allocation = 0.60
            self._state.spear_allocation = 0.40

            # Record event
            event = CircuitBreakerEvent(
                timestamp=time.time(),
                event_type="reset",
                level=ProtectionLevel.NORMAL,
                reason=f"Auto-reset after {time_since_trigger/60:.0f} minutes",
                capital=self._config.CURRENT_CAPITAL,
                drawdown=self.get_drawdown(),
            )
            self._events.append(event)

            logger.info(f"Circuit Breaker RESET: {old_level.name} → NORMAL")
            logger.info(f"  Shield: {self._state.shield_allocation*100:.0f}%, Spear: {self._state.spear_allocation*100:.0f}%")

            self._save_state()

    def _apply_level_adjustments(self, level: ProtectionLevel):
        """Apply strategy allocation adjustments based on protection level."""
        old_shield = self._state.shield_allocation
        old_spear = self._state.spear_allocation

        if level == ProtectionLevel.WARNING:
            # Reduce Spear by 50%
            self._state.spear_allocation = max(0.20, self._state.spear_allocation * 0.5)
            self._state.shield_allocation = 1.0 - self._state.spear_allocation
        elif level == ProtectionLevel.CAUTION:
            # Halt Spear completely
            self._state.spear_allocation = 0.0
            self._state.shield_allocation = 1.0
        elif level == ProtectionLevel.EMERGENCY:
            # All trading halted
            self._state.spear_allocation = 0.0
            self._state.shield_allocation = 0.0

        if old_shield != self._state.shield_allocation or old_spear != self._state.spear_allocation:
            logger.warning(f"Allocation adjusted: Shield {old_shield*100:.0f}%→{self._state.shield_allocation*100:.0f}%, Spear {old_spear*100:.0f}%→{self._state.spear_allocation*100:.0f}%")

    def update_capital(self, new_capital: float):
        """Update current capital and track peak."""
        with self._lock:
            old_capital = self._config.CURRENT_CAPITAL
            self._config.CURRENT_CAPITAL = new_capital

            # Update peak if we've reached a new high
            if new_capital > self._config.PEAK_CAPITAL:
                self._config.PEAK_CAPITAL = new_capital

            # Calculate volatility (absolute change)
            if old_capital > 0:
                volatility = abs(new_capital - old_capital) / old_capital
                self._volatility_samples.append(volatility)
                if len(self._volatility_samples) > 100:
                    self._volatility_samples.pop(0)

            self._save_state()

    def can_trade_wallet(self, wallet_address: str) -> Tuple[bool, str]:
        """Check if we can trade a specific wallet."""
        with self._lock:
            # Check if wallet is blacklisted
            if wallet_address in self._state.wallet_blacklist:
                blacklist_until = self._state.wallet_blacklist[wallet_address]
                if time.time() < blacklist_until:
                    remaining = int(blacklist_until - time.time())
                    return False, f"Wallet blacklisted for {remaining}s"
                else:
                    # Remove from blacklist
                    del self._state.wallet_blacklist[wallet_address]

            # Check overall circuit breaker state
            can_trade, level, reason = self.check_circuit_breaker()
            if not can_trade:
                return False, f"Circuit breaker: {reason}"

            return True, "OK"

    def blacklist_wallet(self, wallet_address: str, reason: str):
        """Blacklist a wallet from trading."""
        with self._lock:
            blacklist_until = time.time() + self._config.WALLET_BLACKLIST_DURATION
            self._state.wallet_blacklist[wallet_address] = blacklist_until

            event = CircuitBreakerEvent(
                timestamp=time.time(),
                event_type="wallet_blacklist",
                level=self._state.current_level,
                reason=reason,
                capital=self._config.CURRENT_CAPITAL,
                drawdown=self.get_drawdown(),
                metadata={"wallet": wallet_address}
            )
            self._events.append(event)

            logger.warning(f"Wallet blacklisted: {wallet_address[:8]}... - {reason}")

    def record_trade_result(self, success: bool, wallet_address: Optional[str] = None):
        """Record a trade result for statistics."""
        with self._lock:
            self._state.daily_trades += 1
            if success:
                self._state.daily_success += 1
            else:
                self._state.daily_failures += 1

                # Check if wallet should be blacklisted
                if wallet_address:
                    # Track consecutive failures per wallet
                    key = f"failures_{wallet_address}"
                    if not hasattr(self, '_wallet_failures'):
                        self._wallet_failures = {}
                    self._wallet_failures[key] = self._wallet_failures.get(key, 0) + 1

                    if self._wallet_failures[key] >= self._config.MAX_WALLET_FAILURES:
                        self.blacklist_wallet(wallet_address, f"{self._wallet_failures[key]} consecutive failures")
                        self._wallet_failures[key] = 0

    def get_current_allocation(self) -> Tuple[float, float]:
        """Get current Shield/Spear allocation."""
        with self._lock:
            return self._state.shield_allocation, self._state.spear_allocation

    def get_aggressive_allocation(self, capital: float) -> Tuple[float, float]:
        """
        Get aggressive allocation based on capital growth stage (user-selected strategy).

        Allocation rules (AGGRESSIVE):
        - <$300: 60% Shield / 40% Spear
        - $300-500: 50% Shield / 50% Spear
        - $500-800: 40% Shield / 60% Spear
        - $800+: 30% Shield / 70% Spear
        """
        if capital < 300:
            return 0.60, 0.40
        elif capital < 500:
            return 0.50, 0.50
        elif capital < 800:
            return 0.40, 0.60
        else:
            return 0.30, 0.70

    def adjust_for_growth_stage(self):
        """Adjust allocation based on current growth stage."""
        if not self._config.AGGRESSIVE_MODE:
            return

        with self._lock:
            # Only adjust if in normal mode
            if self._state.current_level != ProtectionLevel.NORMAL:
                return

            # Get aggressive allocation for current capital
            shield, spear = self.get_aggressive_allocation(self._config.CURRENT_CAPITAL)

            if shield != self._state.shield_allocation or spear != self._state.spear_allocation:
                old_shield = self._state.shield_allocation
                old_spear = self._state.spear_allocation
                self._state.shield_allocation = shield
                self._state.spear_allocation = spear
                self._state.last_adjustment_time = time.time()

                logger.info(f"Growth stage allocation adjusted:")
                logger.info(f"  Capital: ${self._config.CURRENT_CAPITAL:.0f}")
                logger.info(f"  Shield: {old_shield*100:.0f}% → {shield*100:.0f}%")
                logger.info(f"  Spear: {old_spear*100:.0f}% → {spear*100:.0f}%")

                self._save_state()

    def get_status_report(self) -> Dict[str, Any]:
        """Get comprehensive status report."""
        with self._lock:
            drawdown = self.get_drawdown()
            volatility_multiplier = self.get_volatility_multiplier() if self._config.VOLATILITY_MULTIPLIER else 1.0

            return {
                "current_level": self._state.current_level.name,
                "current_state": self._state.current_state.value,
                "starting_capital": self._config.STARTING_CAPITAL,
                "current_capital": self._config.CURRENT_CAPITAL,
                "peak_capital": self._config.PEAK_CAPITAL,
                "drawdown_pct": drawdown * 100,
                "shield_allocation": self._state.shield_allocation * 100,
                "spear_allocation": self._state.spear_allocation * 100,
                "volatility_multiplier": volatility_multiplier,
                "daily_trades": self._state.daily_trades,
                "daily_success": self._state.daily_success,
                "daily_failures": self._state.daily_failures,
                "success_rate": self._state.daily_success / max(1, self._state.daily_trades),
                "blacklisted_wallets": len(self._state.wallet_blacklist),
                "triggered_at": self._state.triggered_at,
                "events_today": len([e for e in self._events if time.time() - e.timestamp < 86400]),
            }

    def print_status_report(self):
        """Print a comprehensive status report."""
        status = self.get_status_report()

        print("\n" + "="*70)
        print("CIRCUIT BREAKER - STATUS REPORT")
        print("="*70)

        print(f"\nProtection Level: {status['current_level']}")
        print(f"State: {status['current_state']}")

        print(f"\nCapital:")
        print(f"  Starting: ${status['starting_capital']:.2f}")
        print(f"  Current: ${status['current_capital']:.2f}")
        print(f"  Peak: ${status['peak_capital']:.2f}")
        print(f"  Drawdown: {status['drawdown_pct']:.1f}%")

        print(f"\nStrategy Allocation:")
        print(f"  Shield: {status['shield_allocation']:.0f}%")
        print(f"  Spear: {status['spear_allocation']:.0f}%")

        print(f"\nTrading Statistics:")
        print(f"  Daily trades: {status['daily_trades']}")
        print(f"  Success: {status['daily_success']}")
        print(f"  Failures: {status['daily_failures']}")
        print(f"  Success rate: {status['success_rate']*100:.1f}%")

        print(f"\nProtection:")
        print(f"  Blacklisted wallets: {status['blacklisted_wallets']}")
        print(f"  Volatility multiplier: {status['volatility_multiplier']:.2f}x")
        print(f"  Events today: {status['events_today']}")

        if status['triggered_at']:
            triggered_ago = (time.time() - status['triggered_at']) / 60
            print(f"\nLast triggered: {triggered_ago:.0f} minutes ago")

        print("="*70 + "\n")

    def reset_daily_counters(self):
        """Reset daily statistics counters."""
        with self._lock:
            self._state.daily_trades = 0
            self._state.daily_success = 0
            self._state.daily_failures = 0
            logger.info("Daily counters reset")


# Global singleton instance
_circuit_breaker: Optional[CircuitBreaker] = None
_breaker_lock = threading.Lock()


def get_circuit_breaker() -> CircuitBreaker:
    """Get the global circuit breaker singleton."""
    global _circuit_breaker

    with _breaker_lock:
        if _circuit_breaker is None:
            _circuit_breaker = CircuitBreaker()

    return _circuit_breaker


def reset_circuit_breaker():
    """Reset the global circuit breaker (mainly for testing)."""
    global _circuit_breaker

    with _breaker_lock:
        if _circuit_breaker:
            del _circuit_breaker
        _circuit_breaker = None


if __name__ == "__main__":
    # Test the circuit breaker
    breaker = get_circuit_breaker()

    # Simulate some capital changes
    print("Testing circuit breaker with capital changes...")

    breaker.update_capital(200)
    can_trade, level, reason = breaker.check_circuit_breaker()
    print(f"Capital $200: can_trade={can_trade}, level={level}, reason={reason}")

    breaker.update_capital(185)  # 7.5% drawdown
    can_trade, level, reason = breaker.check_circuit_breaker()
    print(f"Capital $185: can_trade={can_trade}, level={level}, reason={reason}")

    breaker.update_capital(170)  # 15% drawdown
    can_trade, level, reason = breaker.check_circuit_breaker()
    print(f"Capital $170: can_trade={can_trade}, level={level}, reason={reason}")

    breaker.print_status_report()
