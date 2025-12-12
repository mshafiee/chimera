"""
Chimera Scout Core Module

Provides wallet analysis, WQS calculation, backtesting, and database output functionality.
"""

import sys as _sys

# ---------------------------------------------------------------------------
# Import aliasing for test/runtime compatibility
#
# This repo historically imports this package as both `core.*` and `scout.core.*`.
# Without aliasing, Python can load the same files twice under different names,
# creating *different* Enum/classes (breaking comparisons like TradeAction.SELL).
# ---------------------------------------------------------------------------

if __name__ == "core":
    _sys.modules.setdefault("scout.core", _sys.modules[__name__])
elif __name__ == "scout.core":
    _sys.modules.setdefault("core", _sys.modules[__name__])

from .analyzer import WalletAnalyzer
from .backtester import BacktestSimulator
from .db_writer import RosterWriter, WalletRecord, write_roster_atomic
from .birdeye_client import BirdeyeClient
from .liquidity_collector import LiquidityCollector
from .liquidity import LiquidityProvider
from .models import (
    LiquidityData,
    BacktestConfig,
    HistoricalTrade,
    SimulatedResult,
    SimulatedTrade,
    TradeAction,
    ValidationResult,
    ValidationStatus,
)
from .validator import PrePromotionValidator, PromotionCriteria, validate_wallet_for_promotion
from .wqs import WalletMetrics, calculate_wqs, classify_wallet

# Alias submodules as well (core.<x> <-> scout.core.<x>)
_this_pkg = __name__  # "core" or "scout.core"
_other_pkg = "scout.core" if _this_pkg == "core" else "core"
for _sub in [
    "analyzer",
    "backtester",
    "birdeye_client",
    "db_writer",
    "helius_client",
    "liquidity",
    "liquidity_collector",
    "models",
    "validator",
    "wqs",
]:
    _a = f"{_this_pkg}.{_sub}"
    _b = f"{_other_pkg}.{_sub}"
    if _a in _sys.modules:
        _sys.modules.setdefault(_b, _sys.modules[_a])

__all__ = [
    # Analyzer
    "WalletAnalyzer",
    # Backtester
    "BacktestSimulator",
    # DB Writer
    "RosterWriter",
    "WalletRecord",
    "write_roster_atomic",
    # Liquidity
    "LiquidityProvider",
    "LiquidityData",
    # Models
    "BacktestConfig",
    "HistoricalTrade",
    "SimulatedResult",
    "SimulatedTrade",
    "TradeAction",
    "ValidationResult",
    "ValidationStatus",
    # Validator
    "PrePromotionValidator",
    "PromotionCriteria",
    "validate_wallet_for_promotion",
    # WQS
    "WalletMetrics",
    "calculate_wqs",
    "classify_wallet",
    # Historical Liquidity (optional)
    "BirdeyeClient",
    "LiquidityCollector",
]
