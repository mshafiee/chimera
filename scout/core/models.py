"""
Data models for Scout backtesting and trade analysis.

This module defines the core data structures used throughout the Scout
for representing historical trades, simulation results, and validation outcomes.
"""

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import List, Optional


class TradeAction(Enum):
    """Trade action type."""
    BUY = "BUY"
    SELL = "SELL"


class ValidationStatus(Enum):
    """Pre-promotion validation status."""
    PASSED = "PASSED"
    FAILED_LIQUIDITY = "FAILED_LIQUIDITY"
    FAILED_SLIPPAGE = "FAILED_SLIPPAGE"
    FAILED_NEGATIVE_PNL = "FAILED_NEGATIVE_PNL"
    FAILED_INSUFFICIENT_TRADES = "FAILED_INSUFFICIENT_TRADES"
    ERROR = "ERROR"


@dataclass
class HistoricalTrade:
    """
    Represents a historical trade made by a wallet.
    
    Used for backtesting to simulate what would happen if we copied
    this trade under current market conditions.
    """
    token_address: str
    token_symbol: str
    action: TradeAction
    amount_sol: float
    price_at_trade: float
    timestamp: datetime
    tx_signature: str
    
    # Optional fields that may be populated from historical data
    liquidity_at_trade_usd: Optional[float] = None
    pnl_sol: Optional[float] = None  # Actual PnL if this was a closing trade
    
    def __post_init__(self):
        """Convert string action to enum if needed."""
        if isinstance(self.action, str):
            self.action = TradeAction(self.action.upper())


@dataclass
class LiquidityCheck:
    """Result of a liquidity check for a specific trade."""
    token_address: str
    token_symbol: str
    historical_liquidity_usd: Optional[float]
    current_liquidity_usd: Optional[float]
    required_liquidity_usd: float
    passed: bool
    reason: Optional[str] = None


@dataclass
class SlippageEstimate:
    """Estimated slippage for a trade."""
    token_address: str
    trade_size_sol: float
    liquidity_usd: float
    estimated_slippage_percent: float
    slippage_cost_sol: float
    acceptable: bool  # True if within max_slippage threshold


@dataclass
class SimulatedTrade:
    """
    Result of simulating a single historical trade.
    
    Contains both the original trade data and the simulated outcome
    under current market conditions.
    """
    original_trade: HistoricalTrade
    
    # Liquidity analysis
    current_liquidity_usd: float
    liquidity_sufficient: bool
    
    # Slippage analysis
    estimated_slippage_percent: float
    slippage_cost_sol: float
    
    # Fee analysis
    fee_cost_sol: float
    
    # Final outcome
    simulated_pnl_sol: float
    rejected: bool
    rejection_reason: Optional[str] = None


@dataclass
class SimulatedResult:
    """
    Complete result of backtesting a wallet's historical trades.
    
    This is the output of the BacktestSimulator and determines
    whether a wallet should be promoted to ACTIVE status.
    """
    wallet_address: str
    
    # Trade counts
    total_trades: int
    simulated_trades: int
    rejected_trades: int
    
    # PnL analysis
    original_pnl_sol: float
    simulated_pnl_sol: float
    pnl_difference_sol: float
    
    # Cost breakdown
    total_slippage_cost_sol: float
    total_fee_cost_sol: float
    
    # Rejected trade details
    rejected_trade_details: List[str] = field(default_factory=list)
    
    # Overall result
    passed: bool = False
    failure_reason: Optional[str] = None
    
    @property
    def pnl_reduction_percent(self) -> float:
        """Calculate percentage reduction in PnL due to simulation."""
        if self.original_pnl_sol <= 0:
            return 0.0
        return ((self.original_pnl_sol - self.simulated_pnl_sol) / self.original_pnl_sol) * 100


@dataclass
class ValidationResult:
    """
    Result of pre-promotion validation for a wallet.
    
    This combines backtest results with other validation checks
    to make a final promotion decision.
    """
    wallet_address: str
    status: ValidationStatus
    
    # Backtest results (if performed)
    backtest_result: Optional[SimulatedResult] = None
    
    # Summary
    passed: bool = False
    reason: Optional[str] = None
    
    # Recommendations
    recommended_status: str = "CANDIDATE"  # 'ACTIVE', 'CANDIDATE', or 'REJECTED'
    notes: Optional[str] = None
    
    # Metadata
    validated_at: datetime = field(default_factory=datetime.utcnow)


@dataclass 
class BacktestConfig:
    """Configuration for backtesting simulation."""
    
    # Liquidity thresholds (USD)
    min_liquidity_shield_usd: float = 10000.0
    min_liquidity_spear_usd: float = 5000.0
    
    # Fee configuration
    dex_fee_percent: float = 0.003  # 0.3% typical DEX fee
    
    # Slippage configuration
    max_slippage_percent: float = 0.05  # 5% max acceptable slippage
    slippage_model: str = "sqrt"  # 'sqrt' or 'linear'
    
    # Lookback period
    lookback_days: int = 30
    
    # Minimum requirements
    min_trades_required: int = 5
    
    # Strategy-specific settings
    shield_multiplier: float = 1.0  # Conservative multiplier for Shield
    spear_multiplier: float = 1.5  # More aggressive for Spear
    
    def get_min_liquidity(self, strategy: str) -> float:
        """Get minimum liquidity for a strategy type."""
        if strategy.upper() == "SHIELD":
            return self.min_liquidity_shield_usd
        elif strategy.upper() == "SPEAR":
            return self.min_liquidity_spear_usd
        else:
            return self.min_liquidity_shield_usd  # Default to conservative


# Example usage
if __name__ == "__main__":
    # Create a sample historical trade
    trade = HistoricalTrade(
        token_address="DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
        token_symbol="BONK",
        action=TradeAction.BUY,
        amount_sol=0.5,
        price_at_trade=0.000012,
        timestamp=datetime.utcnow(),
        tx_signature="5xyzABC123...",
        liquidity_at_trade_usd=150000.0,
    )
    
    print(f"Trade: {trade.action.value} {trade.amount_sol} SOL of {trade.token_symbol}")
    print(f"Historical liquidity: ${trade.liquidity_at_trade_usd:,.0f}")
