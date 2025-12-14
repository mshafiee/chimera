"""
Data models for Scout backtesting and trade analysis.

This module defines the core data structures used throughout the Scout
for representing historical trades, simulation results, and validation outcomes.
"""

from dataclasses import dataclass, field
from datetime import datetime
from decimal import Decimal
from enum import Enum
from typing import List, Optional


class TradeAction(Enum):
    """Trade action type."""
    BUY = "BUY"
    SELL = "SELL"


class TraderArchetype(Enum):
    """Trader behavioral archetype classification."""
    SNIPER = "SNIPER"       # Buys < 2 mins after launch, sells < 30 mins
    SWING = "SWING"         # Holds > 4 hours
    SCALPER = "SCALPER"     # Many trades, small timeframe (default)
    INSIDER = "INSIDER"     # Fresh wallet, buys right before pumps
    WHALE = "WHALE"         # Trade size > 50 SOL


@dataclass
class LiquidityData:
    """Snapshot of token liquidity at a point in time."""
    token_address: str
    liquidity_usd: Decimal
    price_usd: Decimal
    volume_24h_usd: Decimal
    timestamp: datetime
    source: str = "unknown"
    # New: Token creation time for sniper checks
    token_creation_timestamp: Optional[datetime] = None


@dataclass
class WalletRecord:
    """Record of a wallet analyzed by the scout."""
    address: str
    status: str  # CANDIDATE, ACTIVE, REJECTED
    wqs_score: float  # Statistical metric, float is acceptable
    roi_7d: float  # Statistical metric, float is acceptable
    roi_30d: float  # Statistical metric, float is acceptable
    trade_count_30d: int
    win_rate: float  # Statistical metric, float is acceptable
    max_drawdown_30d: float  # Statistical metric, float is acceptable
    avg_trade_size_sol: Decimal
    avg_win_sol: Optional[Decimal] = None
    avg_loss_sol: Optional[Decimal] = None
    profit_factor: Optional[float] = None  # Statistical metric, float is acceptable
    realized_pnl_30d_sol: Optional[Decimal] = None
    last_trade_at: Optional[str] = None
    notes: Optional[str] = None
    created_at: str = datetime.utcnow().isoformat()
    # New fields for detailed records
    avg_entry_delay_seconds: Optional[float] = None  # Time metric, float is acceptable
    archetype: Optional[str] = None  # TraderArchetype as string (SNIPER, SWING, SCALPER, INSIDER, WHALE)


class ValidationStatus(Enum):
    """Pre-promotion validation status."""
    PASSED = "PASSED"
    FAILED_LIQUIDITY = "FAILED_LIQUIDITY"
    FAILED_SLIPPAGE = "FAILED_SLIPPAGE"
    FAILED_NEGATIVE_PNL = "FAILED_NEGATIVE_PNL"
    FAILED_INSUFFICIENT_TRADES = "FAILED_INSUFFICIENT_TRADES"
    FAILED_WQS = "FAILED_WQS"  # WQS score below threshold
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
    amount_sol: Decimal
    price_at_trade: Decimal
    timestamp: datetime
    tx_signature: str
    
    # Optional fields that may be populated from historical data
    liquidity_at_trade_usd: Optional[Decimal] = None
    pnl_sol: Optional[Decimal] = None  # Actual PnL if this was a closing trade

    # Additional optional fields for robust swap parsing / PnL derivation
    token_amount: Optional[Decimal] = None  # Token units bought/sold (UI units)
    sol_amount: Optional[Decimal] = None  # SOL spent/received for this swap (positive)
    price_sol: Optional[Decimal] = None  # SOL per token at execution time
    price_usd: Optional[Decimal] = None  # USD per token at execution time (if available)
    
    def __post_init__(self):
        """Convert string action to enum if needed."""
        if isinstance(self.action, str):
            self.action = TradeAction(self.action.upper())


@dataclass
class LiquidityCheck:
    """Result of a liquidity check for a specific trade."""
    token_address: str
    token_symbol: str
    historical_liquidity_usd: Optional[Decimal]
    current_liquidity_usd: Optional[Decimal]
    required_liquidity_usd: Decimal
    passed: bool
    reason: Optional[str] = None


@dataclass
class SlippageEstimate:
    """Estimated slippage for a trade."""
    token_address: str
    trade_size_sol: Decimal
    liquidity_usd: Decimal
    estimated_slippage_percent: Decimal  # Percentage as Decimal for precision
    slippage_cost_sol: Decimal
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
    current_liquidity_usd: Decimal
    liquidity_sufficient: bool
    
    # Slippage analysis
    estimated_slippage_percent: Decimal  # Percentage as Decimal for precision
    slippage_cost_sol: Decimal
    
    # Fee analysis
    fee_cost_sol: Decimal
    
    # Final outcome
    simulated_pnl_sol: Decimal
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
    original_pnl_sol: Decimal
    simulated_pnl_sol: Decimal
    pnl_difference_sol: Decimal
    
    # Cost breakdown
    total_slippage_cost_sol: Decimal
    total_fee_cost_sol: Decimal
    
    # Rejected trade details
    rejected_trade_details: List[str] = field(default_factory=list)
    
    # Overall result
    passed: bool = False
    failure_reason: Optional[str] = None
    
    @property
    def pnl_reduction_percent(self) -> float:
        """Calculate percentage reduction in PnL due to simulation."""
        if self.original_pnl_sol <= Decimal('0'):
            return 0.0
        original = float(self.original_pnl_sol)
        simulated = float(self.simulated_pnl_sol)
        return ((original - simulated) / original) * 100


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
    min_liquidity_shield_usd: Decimal = field(default_factory=lambda: Decimal('10000.0'))
    min_liquidity_spear_usd: Decimal = field(default_factory=lambda: Decimal('5000.0'))
    
    # Fee configuration
    dex_fee_percent: Decimal = field(default_factory=lambda: Decimal('0.003'))  # 0.3% typical DEX fee

    # Execution costs (SOL-denominated, per swap) to better match Operator reality.
    #
    # These are intentionally simple knobs; if you want a more accurate model,
    # wire in tip estimation (percentile) + RPC/compute-budget fee estimation.
    # Realistic execution costs for backtesting (critical for hype tokens)
    priority_fee_sol_per_trade: Decimal = field(default_factory=lambda: Decimal('0.0005'))
    jito_tip_sol_per_trade: Decimal = field(default_factory=lambda: Decimal('0.0005'))
    
    # Slippage configuration
    max_slippage_percent: Decimal = field(default_factory=lambda: Decimal('0.05'))  # 5% max acceptable slippage
    
    # Lookback period
    lookback_days: int = 30
    
    # Minimum requirements
    min_trades_required: int = 5
    
    # Strategy-specific settings
    shield_multiplier: Decimal = field(default_factory=lambda: Decimal('1.0'))  # Conservative multiplier for Shield
    spear_multiplier: Decimal = field(default_factory=lambda: Decimal('1.5'))  # More aggressive for Spear

    # Copy-viability gate (PDD):
    # If enabled, reject wallets whose traded tokens no longer meet current liquidity
    # thresholds (i.e., the token is effectively dead/un-copyable now).
    #
    # Default is False to keep unit tests deterministic and to avoid surprising
    # network calls in offline environments. Enable in production Scout runs.
    enforce_current_liquidity: bool = False
    
    def get_min_liquidity(self, strategy: str) -> Decimal:
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
        amount_sol=Decimal('0.5'),
        price_at_trade=Decimal('0.000012'),
        timestamp=datetime.utcnow(),
        tx_signature="5xyzABC123...",
        liquidity_at_trade_usd=Decimal('150000.0'),
    )
    
    print(f"Trade: {trade.action.value} {trade.amount_sol} SOL of {trade.token_symbol}")
    print(f"Historical liquidity: ${trade.liquidity_at_trade_usd:,.0f}")
