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
elif __name__ == "scout.core":  # pragma: no cover - mutually exclusive alias, exercised only when imported as scout.core
    _sys.modules.setdefault("core", _sys.modules[__name__])

from .analyzer import WalletAnalyzer
from .backtester import BacktestSimulator
from .roster_writer_db import WalletRecord, write_wallets_to_db
from .birdeye_client import BirdeyeClient
from .helius_client import HeliusClient
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


def _alias_submodule(_sub: str) -> None:
    _a = f"{_this_pkg}.{_sub}"
    _b = f"{_other_pkg}.{_sub}"
    if _a in _sys.modules:
        _sys.modules.setdefault(_b, _sys.modules[_a])


for _sub in [
    "analyzer",
    "backtester",
    "birdeye_client",
    "helius_client",
    "liquidity",
    "models",
    "roster_writer_db",
    "validator",
    "wqs",
]:
    _alias_submodule(_sub)

# Mirror every other submodule loaded during this import (db, utils,
# advanced_cache, caching, helius_credit_tracker, ...) so whichever name
# is used first becomes the single canonical module.
for _mod_name in list(_sys.modules):
    if _mod_name.startswith(_this_pkg + "."):
        _sys.modules.setdefault(
            _other_pkg + _mod_name[len(_this_pkg):],
            _sys.modules[_mod_name],
        )

__all__ = [
    # Analyzer
    "WalletAnalyzer",
    # Backtester
    "BacktestSimulator",
    # DB Writer
    "WalletRecord",
    "write_wallets_to_db",
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
    # Helius (imported so the submodule alias is guaranteed)
    "HeliusClient",
]
