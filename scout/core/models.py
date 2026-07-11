"""
Data models for Scout backtesting and trade analysis.

This module defines the core data structures used throughout the Scout
for representing historical trades, simulation results, and validation outcomes.
"""

from dataclasses import dataclass, field
from datetime import datetime

from .utils import utcnow

from decimal import Decimal
from enum import Enum
from typing import Dict, List, Optional


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
    created_at: str = field(default_factory=lambda: utcnow().isoformat())
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

    # Individual simulated trade results (populated by BacktestSimulator)
    trades: List = field(default_factory=list)

    # Overall result
    passed: bool = False
    failure_reason: Optional[str] = None

    # Market regime during the wallet's trade period
    regime_risk: Optional[str] = None  # "BULL", "BEAR", "SIDEWAYS", or None

    # Final position state after simulation (token_address -> {qty, cost_basis_sol})
    # Used by walk-forward to carry positions from train to test phase.
    final_positions: Dict[str, Dict[str, Decimal]] = field(default_factory=dict)
    
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
    priority_fee_sol_per_trade: Decimal = field(default_factory=lambda: Decimal('0.00005'))
    jito_tip_sol_per_trade: Decimal = field(default_factory=lambda: Decimal('0.0001'))

    # Time-delay slippage: the 100-500ms operator latency + block inclusion delay
    # causes price movement between signal observation and trade execution.
    # These percentages model the expected adverse price movement per leg.
    # NOTE: These are base values; backtester.py applies 1x-10x turnover multiplier.
    entry_delay_slippage_pct: Decimal = field(default_factory=lambda: Decimal('0.015'))  # 1.5% base Shield default
    exit_delay_slippage_pct: Decimal = field(default_factory=lambda: Decimal('0.010'))   # 1.0% base Shield default

    # MEV/sandwich penalty applied to SELL trades to model sandwich attacks
    # on copied exits. Derived from empirical sandwich attack rates on Solana.
    mev_penalty_pct: Decimal = field(default_factory=lambda: Decimal('0.002'))  # 0.2% Shield default

    # When True, dynamic priority fees are fetched from Helius at run time
    # using getPriorityFeeEstimate; otherwise the static priority_fee_sol_per_trade is used.
    use_dynamic_fees: bool = False

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
    # Reject wallets whose traded tokens no longer meet current liquidity thresholds
    # (i.e., the token is effectively dead/un-copyable now).
    # Only fires when liquidity provider is in "real" mode, so offline/test
    # environments running in "simulated" mode are unaffected.
    enforce_current_liquidity: bool = True
    
    # Copier size override: when set, use this trade size (SOL) for slippage
    # and cost estimation instead of the original trader's size. Keeps the
    # original size for PnL ratio computation. Defaults to None (use original).
    simulate_at_size_sol: Optional[Decimal] = None

    # Time-decay weighting: when enabled, recent trades are weighted more heavily
    # in the simulated PnL aggregation, reflecting that recent performance is
    # more predictive of future results.
    backtest_time_decay_enabled: bool = False
    backtest_time_decay_half_life_days: int = 14
    
    def __post_init__(self):
        """Coerce all Decimal fields so tests can pass plain floats/ints."""
        def _d(v):
            return Decimal(str(v)) if not isinstance(v, Decimal) else v
        self.min_liquidity_shield_usd = _d(self.min_liquidity_shield_usd)
        self.min_liquidity_spear_usd = _d(self.min_liquidity_spear_usd)
        self.dex_fee_percent = _d(self.dex_fee_percent)
        self.max_slippage_percent = _d(self.max_slippage_percent)
        self.priority_fee_sol_per_trade = _d(self.priority_fee_sol_per_trade)
        self.jito_tip_sol_per_trade = _d(self.jito_tip_sol_per_trade)
        self.entry_delay_slippage_pct = _d(self.entry_delay_slippage_pct)
        self.exit_delay_slippage_pct = _d(self.exit_delay_slippage_pct)
        self.mev_penalty_pct = _d(self.mev_penalty_pct)
        self.shield_multiplier = _d(self.shield_multiplier)
        self.spear_multiplier = _d(self.spear_multiplier)

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
        timestamp=utcnow(),
        tx_signature="5xyzABC123...",
        liquidity_at_trade_usd=Decimal('150000.0'),
    )
    
    print(f"Trade: {trade.action.value} {trade.amount_sol} SOL of {trade.token_symbol}")
    print(f"Historical liquidity: ${trade.liquidity_at_trade_usd:,.0f}")
