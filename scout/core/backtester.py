"""
Backtesting Simulator for Scout wallet validation.

This module simulates historical trades under current market conditions
to determine if a wallet's past performance can be replicated.

Key features:
- Historical liquidity validation
- Slippage estimation based on trade size vs liquidity
- Fee calculation
- PnL comparison (original vs simulated)

A wallet FAILS backtest if:
- Current liquidity < minimum threshold for any trade
- Simulated PnL < 0 after slippage and fees
- Too many trades would be rejected due to liquidity
"""

from dataclasses import dataclass
from datetime import datetime
from decimal import Decimal
from typing import Dict, List, Optional, Tuple
import logging

from .models import (
    BacktestConfig,
    HistoricalTrade,
    SimulatedResult,
    SimulatedTrade,
    TradeAction,
    LiquidityData,
)
from .liquidity import LiquidityProvider
from .decimal_utils import float_to_decimal, decimal_to_float, safe_decimal_divide


logger = logging.getLogger(__name__)


class BacktestSimulator:
    """
    Simulates historical trades under current market conditions.
    
    Usage:
        simulator = BacktestSimulator(liquidity_provider, config)
        result = simulator.simulate_wallet(wallet_address, trades)
        if result.passed:
            print("Wallet passed backtest - eligible for promotion")
    """
    
    def __init__(
        self,
        liquidity_provider: LiquidityProvider,
        config: Optional[BacktestConfig] = None,
    ):
        """
        Initialize the backtester.
        
        Args:
            liquidity_provider: Provider for liquidity data
            config: Backtest configuration (uses defaults if None)
        """
        self.liquidity = liquidity_provider
        self.config = config or BacktestConfig()
    
    def simulate_wallet(
        self,
        wallet_address: str,
        trades: List[HistoricalTrade],
        strategy: str = "SHIELD",
    ) -> SimulatedResult:
        """
        Simulate all historical trades for a wallet using round-trip cashflow model.
        
        This tracks positions per token and computes realized PnL only on SELL trades,
        applying costs realistically at both entry (BUY) and exit (SELL).
        
        Args:
            wallet_address: Wallet address being validated
            trades: List of historical trades (should be sorted chronologically)
            strategy: Strategy type ('SHIELD' or 'SPEAR')
            
        Returns:
            SimulatedResult with pass/fail and details
        """
        if not trades:
            return SimulatedResult(
                wallet_address=wallet_address,
                total_trades=0,
                simulated_trades=0,
                rejected_trades=0,
                original_pnl_sol=0.0,
                simulated_pnl_sol=0.0,
                pnl_difference_sol=0.0,
                total_slippage_cost_sol=0.0,
                total_fee_cost_sol=0.0,
                passed=False,
                failure_reason="No trades to simulate",
            )
        
        # Sort trades chronologically for position tracking
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        
        # Track low-confidence liquidity usage (survivorship bias risk)
        low_confidence_trades_count = 0
        
        # Check minimum trades
        insufficient_trades_failure: Optional[str] = None
        if len(sorted_trades) < self.config.min_trades_required:
            insufficient_trades_failure = (
                f"Insufficient trades: {len(sorted_trades)} < {self.config.min_trades_required}"
            )
        
        # Get minimum liquidity threshold for strategy (convert to Decimal)
        min_liquidity_decimal = self.config.get_min_liquidity(strategy)
        sol_price_float = self.liquidity.get_sol_price_usd()
        sol_price = float_to_decimal(sol_price_float)
        
        # Round-trip position tracking: {token_address: {"qty": Decimal, "cost_basis_sol": Decimal}}
        positions: Dict[str, Dict[str, Decimal]] = {}
        
        # Track results
        simulated_trades: List[SimulatedTrade] = []
        rejected_details: List[str] = []
        
        # Track original realized PnL (only from SELL trades with pnl_sol) - using Decimal
        total_original_realized_pnl = Decimal('0')
        # Track simulated realized PnL (only from SELL trades)
        total_simulated_realized_pnl = Decimal('0')
        total_slippage = Decimal('0')
        total_fees = Decimal('0')
        rejected_count = 0
        
        for trade in sorted_trades:
            sim_trade, rejection_reason = self._simulate_trade_roundtrip(
                trade, min_liquidity_decimal, sol_price, positions
            )
            simulated_trades.append(sim_trade)
            
            # Track low-confidence liquidity usage (check if liquidity was from fallback)
            # We check the original trade's liquidity source by re-fetching if needed
            if not trade.liquidity_at_trade_usd:
                # Only check if we had to fetch liquidity (not from trade data)
                liquidity_check = self.liquidity.get_historical_liquidity_or_current(
                    trade.token_address,
                    trade.timestamp,
                )
                if liquidity_check and "fallback_capped" in liquidity_check.source:
                    low_confidence_trades_count += 1
            
            # Track original realized PnL (only SELL trades with pnl_sol)
            if trade.action == TradeAction.SELL and trade.pnl_sol is not None:
                total_original_realized_pnl += trade.pnl_sol
            
            if sim_trade.rejected:
                rejected_count += 1
                rejected_details.append(
                    f"{trade.token_symbol}: {rejection_reason}"
                )
            else:
                # Track costs
                total_slippage += sim_trade.slippage_cost_sol
                total_fees += sim_trade.fee_cost_sol
                
                # Track simulated realized PnL (only SELL trades)
                if trade.action == TradeAction.SELL and sim_trade.simulated_pnl_sol is not None:
                    total_simulated_realized_pnl += sim_trade.simulated_pnl_sol
        
        # Calculate rejection rate
        rejection_rate = rejected_count / len(sorted_trades) if sorted_trades else 0.0
        
        # Determine pass/fail
        passed = True
        failure_reason: Optional[str] = None

        # Fail if insufficient trades
        if insufficient_trades_failure is not None:
            passed = False
            failure_reason = insufficient_trades_failure
        
        # Fail if too many trades rejected (>50%)
        elif passed and rejection_rate > 0.5:
            passed = False
            failure_reason = f"Too many trades rejected: {rejection_rate*100:.0f}%"
        
        # Fail if simulated realized PnL is negative
        elif passed and total_simulated_realized_pnl < Decimal('0'):
            passed = False
            failure_reason = f"Negative simulated realized PnL: {decimal_to_float(total_simulated_realized_pnl):.4f} SOL"
        
        # Fail if PnL reduction is too high (>80% reduction) - only if original was positive
        elif passed and total_original_realized_pnl > Decimal('0'):
            pnl_reduction = safe_decimal_divide(
                total_original_realized_pnl - total_simulated_realized_pnl,
                total_original_realized_pnl
            )
            if pnl_reduction > Decimal('0.8'):
                passed = False
                failure_reason = f"PnL reduction too high: {decimal_to_float(pnl_reduction * Decimal('100')):.0f}%"
        
        # Warn about survivorship bias if significant portion of trades used low-confidence liquidity
        if low_confidence_trades_count > 0:
            low_confidence_ratio = low_confidence_trades_count / len(sorted_trades)
            if low_confidence_ratio > 0.3:  # More than 30% of trades
                logger.warning(
                    f"⚠️  SURVIVORSHIP BIAS RISK: {low_confidence_trades_count}/{len(sorted_trades)} "
                    f"({low_confidence_ratio*100:.0f}%) trades used fallback liquidity data. "
                    f"Backtest results may be inflated for tokens that mooned or filtered for tokens that rugged. "
                    f"Backtest confidence: LOW."
                )
                # Add warning to failure reason if it exists, or create a note
                if failure_reason:
                    failure_reason += f" (Also: {low_confidence_ratio*100:.0f}% trades used low-confidence liquidity)"
                else:
                    # Don't fail, but note the risk
                    logger.info(
                        f"Backtest passed but with survivorship bias risk. "
                        f"Consider requiring Birdeye historical data for production."
                    )
        
        return SimulatedResult(
            wallet_address=wallet_address,
            total_trades=len(sorted_trades),
            simulated_trades=len(sorted_trades) - rejected_count,
            rejected_trades=rejected_count,
            original_pnl_sol=total_original_realized_pnl,  # Only realized PnL (Decimal)
            simulated_pnl_sol=total_simulated_realized_pnl,  # Only realized PnL (Decimal)
            pnl_difference_sol=total_original_realized_pnl - total_simulated_realized_pnl,
            total_slippage_cost_sol=total_slippage,
            total_fee_cost_sol=total_fees,
            rejected_trade_details=rejected_details,
            passed=passed,
            failure_reason=failure_reason,
        )
    
    def _simulate_trade_roundtrip(
        self,
        trade: HistoricalTrade,
        min_liquidity: Decimal,
        sol_price: Decimal,
        positions: Dict[str, Dict[str, Decimal]],
    ) -> Tuple[SimulatedTrade, Optional[str]]:
        """
        Simulate a single trade using round-trip cashflow model.
        
        Tracks positions per token and computes realized PnL only on SELL trades.
        Costs are applied at both entry (BUY) and exit (SELL).
        
        Args:
            trade: Historical trade to simulate
            min_liquidity: Minimum liquidity requirement (USD)
            sol_price: Current SOL price in USD
            positions: Position ledger (mutated in-place)
            
        Returns:
            Tuple of (SimulatedTrade, rejection_reason)
        """
        # Get liquidity data (historical-at-trade if available).
        liquidity_data = None
        low_confidence_liquidity = False
        if trade.liquidity_at_trade_usd is not None:
            liquidity_data = LiquidityData(
                token_address=trade.token_address,
                liquidity_usd=trade.liquidity_at_trade_usd,
                price_usd=0.0,
                volume_24h_usd=0.0,
                timestamp=trade.timestamp,
                source="trade_attached",
            )
        else:
            liquidity_data = self.liquidity.get_historical_liquidity_or_current(
                trade.token_address,
                trade.timestamp,
            )
            # Check if liquidity data is from fallback (low confidence)
            if liquidity_data and "fallback_capped" in liquidity_data.source:
                low_confidence_liquidity = True
                logger.warning(
                    f"Using low-confidence fallback liquidity for trade simulation: "
                    f"{trade.token_address[:8]}... at {trade.timestamp.isoformat()}. "
                    f"Backtest results may be affected by survivorship bias."
                )
        
        if not liquidity_data:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=Decimal('0'),
                liquidity_sufficient=False,
                estimated_slippage_percent=Decimal('1.0'),
                slippage_cost_sol=trade.amount_sol,
                fee_cost_sol=Decimal('0'),
                simulated_pnl_sol=Decimal('0'),
                rejected=True,
                rejection_reason="Could not fetch liquidity data",
            ), "Could not fetch liquidity data"

        # PDD requirement:
        # - Check liquidity at the time of the historical trade (trade-time viability)
        # - ALSO reject if current liquidity is now too low to copy (token is dead)
        #
        # IMPORTANT FOR TESTS / OFFLINE MODE:
        # In simulated mode, `get_current_liquidity()` is intentionally non-deterministic.
        # We therefore enforce the "current liquidity" gate only when the provider is
        # running in real mode (i.e., backed by real data sources).
        historical_liquidity = liquidity_data.liquidity_usd or Decimal('0')

        # Check historical liquidity requirement (at-trade)
        if historical_liquidity < min_liquidity:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=historical_liquidity,
                liquidity_sufficient=False,
                estimated_slippage_percent=Decimal('1.0'),
                slippage_cost_sol=trade.amount_sol,
                fee_cost_sol=Decimal('0'),
                simulated_pnl_sol=Decimal('0'),
                rejected=True,
                rejection_reason=f"Historical liquidity ${decimal_to_float(historical_liquidity):,.0f} < ${decimal_to_float(min_liquidity):,.0f}",
            ), f"Insufficient historical liquidity: ${decimal_to_float(historical_liquidity):,.0f}"

        # Check current liquidity requirement (copyable now) - only when explicitly enabled.
        if (
            getattr(self.liquidity, "mode", "").lower() == "real"
            and getattr(self.config, "enforce_current_liquidity", False)
        ):
            current_liq_data = self.liquidity.get_current_liquidity(trade.token_address)
            if not current_liq_data:
                return SimulatedTrade(
                    original_trade=trade,
                    current_liquidity_usd=historical_liquidity,
                    liquidity_sufficient=False,
                    estimated_slippage_percent=Decimal('1.0'),
                    slippage_cost_sol=trade.amount_sol,
                    fee_cost_sol=Decimal('0'),
                    simulated_pnl_sol=Decimal('0'),
                    rejected=True,
                    rejection_reason="Could not fetch current liquidity",
                ), "Could not fetch current liquidity"

            current_liquidity_now = current_liq_data.liquidity_usd or Decimal('0')
            if current_liquidity_now < min_liquidity:
                return SimulatedTrade(
                    original_trade=trade,
                    current_liquidity_usd=historical_liquidity,
                    liquidity_sufficient=False,
                    estimated_slippage_percent=Decimal('1.0'),
                    slippage_cost_sol=trade.amount_sol,
                    fee_cost_sol=Decimal('0'),
                    simulated_pnl_sol=Decimal('0'),
                    rejected=True,
                    rejection_reason=f"Current liquidity ${decimal_to_float(current_liquidity_now):,.0f} < ${decimal_to_float(min_liquidity):,.0f}",
                ), f"Insufficient current liquidity: ${decimal_to_float(current_liquidity_now):,.0f}"
        
        # Get trade size in SOL (use sol_amount if available, fallback to amount_sol)
        trade_size_sol = trade.sol_amount if trade.sol_amount is not None else trade.amount_sol
        if trade_size_sol <= Decimal('0'):
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=historical_liquidity,
                liquidity_sufficient=True,
                estimated_slippage_percent=Decimal('0'),
                slippage_cost_sol=Decimal('0'),
                fee_cost_sol=Decimal('0'),
                simulated_pnl_sol=Decimal('0'),
                rejected=True,
                rejection_reason="Invalid trade size",
            ), "Invalid trade size"
        
        # Estimate slippage using historical liquidity (trade-time conditions).
        # Convert to float for estimate_slippage (it may still use float internally)
        vol_24h = getattr(liquidity_data, 'volume_24h_usd', Decimal('0'))
        slippage_float = self.liquidity.estimate_slippage(
            trade.token_address,
            decimal_to_float(trade_size_sol),
            decimal_to_float(historical_liquidity),
            decimal_to_float(sol_price),
            volume_24h_usd=decimal_to_float(vol_24h),
        )
        slippage = float_to_decimal(slippage_float)
        
        # Check if slippage is acceptable
        if slippage > self.config.max_slippage_percent:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=historical_liquidity,
                liquidity_sufficient=True,
                estimated_slippage_percent=slippage,
                slippage_cost_sol=trade_size_sol * slippage,
                fee_cost_sol=Decimal('0'),
                simulated_pnl_sol=Decimal('0'),
                rejected=True,
                rejection_reason=f"Slippage {decimal_to_float(slippage * Decimal('100')):.1f}% > {decimal_to_float(self.config.max_slippage_percent * Decimal('100')):.1f}%",
            ), f"Excessive slippage: {decimal_to_float(slippage * Decimal('100')):.1f}%"
        
        # Calculate costs per trade using Decimal
        slippage_cost = trade_size_sol * slippage
        fee_cost = trade_size_sol * self.config.dex_fee_percent
        priority_fee_cost = max(Decimal('0'), self.config.priority_fee_sol_per_trade)
        jito_tip_cost = max(Decimal('0'), self.config.jito_tip_sol_per_trade)
        execution_cost = priority_fee_cost + jito_tip_cost
        total_cost = slippage_cost + fee_cost + execution_cost
        
        # Round-trip position tracking using Decimal
        token = trade.token_address
        position = positions.setdefault(token, {"qty": Decimal('0'), "cost_basis_sol": Decimal('0')})
        
        simulated_pnl = Decimal('0')
        
        if trade.action == TradeAction.BUY:
            # BUY: apply costs, increase position
            net_sol_spent = trade_size_sol + total_cost
            token_qty = trade.token_amount if trade.token_amount is not None else Decimal('0')
            
            # If token_amount not available, estimate from price
            if token_qty <= Decimal('0') and trade.price_sol and trade.price_sol > Decimal('0'):
                token_qty = safe_decimal_divide(trade_size_sol, trade.price_sol)
            
            if token_qty > Decimal('0'):
                position["qty"] += token_qty
                position["cost_basis_sol"] += net_sol_spent
                # No realized PnL on BUY
                simulated_pnl = Decimal('0')
            else:
                # Can't track position without token quantity
                logger.warning(f"BUY trade missing token_amount for {token[:8]}...")
                simulated_pnl = Decimal('0')
        
        elif trade.action == TradeAction.SELL:
            # SELL: compute proceeds, realize PnL, reduce position
            net_sol_received = trade_size_sol - total_cost  # Costs reduce proceeds
            token_qty = trade.token_amount if trade.token_amount is not None else Decimal('0')
            
            # If token_amount not available, estimate from price
            if token_qty <= Decimal('0') and trade.price_sol and trade.price_sol > Decimal('0'):
                token_qty = safe_decimal_divide(trade_size_sol, trade.price_sol)
            
            if token_qty <= Decimal('0'):
                return SimulatedTrade(
                    original_trade=trade,
                    current_liquidity_usd=historical_liquidity,
                    liquidity_sufficient=True,
                    estimated_slippage_percent=slippage,
                    slippage_cost_sol=slippage_cost,
                    fee_cost_sol=fee_cost + execution_cost,
                    simulated_pnl_sol=Decimal('0'),
                    rejected=True,
                    rejection_reason="Missing token quantity for SELL",
                ), "Missing token quantity for SELL"
            
            if position["qty"] <= Decimal('0'):
                # Can't sell what we don't have - this is a data issue
                logger.warning(f"SELL trade without position for {token[:8]}...")
                simulated_pnl = Decimal('0')
            else:
                # Calculate realized PnL
                sell_qty = min(token_qty, position["qty"])
                avg_cost_per_token = safe_decimal_divide(position["cost_basis_sol"], position["qty"])
                allocated_cost_basis = avg_cost_per_token * sell_qty
                
                # Realized PnL = proceeds - allocated cost basis
                simulated_pnl = net_sol_received - allocated_cost_basis
                
                # Reduce position
                position["qty"] -= sell_qty
                position["cost_basis_sol"] -= allocated_cost_basis
                if position["qty"] <= Decimal('0.000000000001'):  # Use Decimal comparison instead of 1e-12
                    positions.pop(token, None)
        
        return SimulatedTrade(
            original_trade=trade,
            current_liquidity_usd=historical_liquidity,
            liquidity_sufficient=True,
            estimated_slippage_percent=slippage,
            slippage_cost_sol=slippage_cost,
            fee_cost_sol=fee_cost + execution_cost,
            simulated_pnl_sol=simulated_pnl,
            rejected=False,
            rejection_reason=None,
        ), None
    
    def _simulate_trade(
        self,
        trade: HistoricalTrade,
        min_liquidity: float,
        sol_price: float,
    ) -> Tuple[SimulatedTrade, Optional[str]]:
        """
        Legacy per-trade simulation (kept for backward compatibility).
        
        For new code, use _simulate_trade_roundtrip instead.
        This method uses a simple per-trade model without position tracking.
        """
        # Convert float parameters to Decimal for internal use
        min_liquidity_decimal = float_to_decimal(min_liquidity)
        sol_price_decimal = float_to_decimal(sol_price)
        # Use empty positions dict for legacy behavior (no position tracking)
        return self._simulate_trade_roundtrip(trade, min_liquidity_decimal, sol_price_decimal, {})
    



# Example usage
if __name__ == "__main__":
    from .liquidity import LiquidityProvider
    
    # Create simulator
    provider = LiquidityProvider()
    config = BacktestConfig(
        min_liquidity_shield_usd=10000,
        min_liquidity_spear_usd=5000,
        dex_fee_percent=0.003,
        max_slippage_percent=0.05,
    )
    simulator = BacktestSimulator(provider, config)
    
    # Create sample trades
    trades = [
        HistoricalTrade(
            token_address="DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
            token_symbol="BONK",
            action=TradeAction.BUY,
            amount_sol=0.5,
            price_at_trade=0.000012,
            timestamp=datetime.utcnow(),
            tx_signature="tx1",
            pnl_sol=0.15,
        ),
        HistoricalTrade(
            token_address="EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm",
            token_symbol="WIF",
            action=TradeAction.BUY,
            amount_sol=0.3,
            price_at_trade=1.5,
            timestamp=datetime.utcnow(),
            tx_signature="tx2",
            pnl_sol=0.08,
        ),
    ]
    
    # Add more trades to meet minimum requirement
    for i in range(5):
        trades.append(HistoricalTrade(
            token_address="DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
            token_symbol="BONK",
            action=TradeAction.SELL,
            amount_sol=0.1,
            price_at_trade=0.000015,
            timestamp=datetime.utcnow(),
            tx_signature=f"tx{i+3}",
            pnl_sol=0.02,
        ))
    
    # Run simulation
    result = simulator.simulate_wallet(
        "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
        trades,
        strategy="SHIELD",
    )
    
    print(f"Backtest Result: {'PASSED' if result.passed else 'FAILED'}")
    print(f"  Total trades: {result.total_trades}")
    print(f"  Simulated: {result.simulated_trades}")
    print(f"  Rejected: {result.rejected_trades}")
    print(f"  Original PnL: {result.original_pnl_sol:.4f} SOL")
    print(f"  Simulated PnL: {result.simulated_pnl_sol:.4f} SOL")
    print(f"  Slippage cost: {result.total_slippage_cost_sol:.4f} SOL")
    print(f"  Fee cost: {result.total_fee_cost_sol:.4f} SOL")
    if result.failure_reason:
        print(f"  Failure reason: {result.failure_reason}")
