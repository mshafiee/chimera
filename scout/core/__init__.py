"""
Chimera Scout Core Module

Provides wallet analysis, WQS calculation, backtesting, and database output functionality.
"""

from .analyzer import WalletAnalyzer
from .backtester import BacktestSimulator
from .db_writer import RosterWriter, WalletRecord, write_roster_atomic
from .liquidity import LiquidityProvider, LiquidityData
from .models import (
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
]
