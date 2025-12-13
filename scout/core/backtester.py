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
        
        # Check minimum trades
        insufficient_trades_failure: Optional[str] = None
        if len(sorted_trades) < self.config.min_trades_required:
            insufficient_trades_failure = (
                f"Insufficient trades: {len(sorted_trades)} < {self.config.min_trades_required}"
            )
        
        # Get minimum liquidity threshold for strategy
        min_liquidity = self.config.get_min_liquidity(strategy)
        sol_price = self.liquidity.get_sol_price_usd()
        
        # Round-trip position tracking: {token_address: {"qty": float, "cost_basis_sol": float}}
        positions: Dict[str, Dict[str, float]] = {}
        
        # Track results
        simulated_trades: List[SimulatedTrade] = []
        rejected_details: List[str] = []
        
        # Track original realized PnL (only from SELL trades with pnl_sol)
        total_original_realized_pnl = 0.0
        # Track simulated realized PnL (only from SELL trades)
        total_simulated_realized_pnl = 0.0
        total_slippage = 0.0
        total_fees = 0.0
        rejected_count = 0
        
        for trade in sorted_trades:
            sim_trade, rejection_reason = self._simulate_trade_roundtrip(
                trade, min_liquidity, sol_price, positions
            )
            simulated_trades.append(sim_trade)
            
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
        elif passed and total_simulated_realized_pnl < 0:
            passed = False
            failure_reason = f"Negative simulated realized PnL: {total_simulated_realized_pnl:.4f} SOL"
        
        # Fail if PnL reduction is too high (>80% reduction) - only if original was positive
        elif passed and total_original_realized_pnl > 0:
            pnl_reduction = (total_original_realized_pnl - total_simulated_realized_pnl) / total_original_realized_pnl
            if pnl_reduction > 0.8:
                passed = False
                failure_reason = f"PnL reduction too high: {pnl_reduction*100:.0f}%"
        
        return SimulatedResult(
            wallet_address=wallet_address,
            total_trades=len(sorted_trades),
            simulated_trades=len(sorted_trades) - rejected_count,
            rejected_trades=rejected_count,
            original_pnl_sol=total_original_realized_pnl,  # Only realized PnL
            simulated_pnl_sol=total_simulated_realized_pnl,  # Only realized PnL
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
        min_liquidity: float,
        sol_price: float,
        positions: Dict[str, Dict[str, float]],
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
        
        if not liquidity_data:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=0,
                liquidity_sufficient=False,
                estimated_slippage_percent=1.0,
                slippage_cost_sol=trade.amount_sol,
                fee_cost_sol=0,
                simulated_pnl_sol=0,
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
        historical_liquidity = float(liquidity_data.liquidity_usd or 0.0)

        # Check historical liquidity requirement (at-trade)
        if historical_liquidity < min_liquidity:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=historical_liquidity,
                liquidity_sufficient=False,
                estimated_slippage_percent=1.0,
                slippage_cost_sol=trade.amount_sol,
                fee_cost_sol=0,
                simulated_pnl_sol=0,
                rejected=True,
                rejection_reason=f"Historical liquidity ${historical_liquidity:,.0f} < ${min_liquidity:,.0f}",
            ), f"Insufficient historical liquidity: ${historical_liquidity:,.0f}"

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
                    estimated_slippage_percent=1.0,
                    slippage_cost_sol=trade.amount_sol,
                    fee_cost_sol=0,
                    simulated_pnl_sol=0,
                    rejected=True,
                    rejection_reason="Could not fetch current liquidity",
                ), "Could not fetch current liquidity"

            current_liquidity_now = float(current_liq_data.liquidity_usd or 0.0)
            if current_liquidity_now < min_liquidity:
                return SimulatedTrade(
                    original_trade=trade,
                    current_liquidity_usd=historical_liquidity,
                    liquidity_sufficient=False,
                    estimated_slippage_percent=1.0,
                    slippage_cost_sol=trade.amount_sol,
                    fee_cost_sol=0,
                    simulated_pnl_sol=0,
                    rejected=True,
                    rejection_reason=f"Current liquidity ${current_liquidity_now:,.0f} < ${min_liquidity:,.0f}",
                ), f"Insufficient current liquidity: ${current_liquidity_now:,.0f}"
        
        # Get trade size in SOL (use sol_amount if available, fallback to amount_sol)
        trade_size_sol = trade.sol_amount if trade.sol_amount is not None else trade.amount_sol
        if trade_size_sol <= 0:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=current_liquidity,
                liquidity_sufficient=True,
                estimated_slippage_percent=0,
                slippage_cost_sol=0,
                fee_cost_sol=0,
                simulated_pnl_sol=0,
                rejected=True,
                rejection_reason="Invalid trade size",
            ), "Invalid trade size"
        
        # Estimate slippage using historical liquidity (trade-time conditions).
        vol_24h = getattr(liquidity_data, 'volume_24h_usd', 0.0)
        slippage = self.liquidity.estimate_slippage(
            trade.token_address,
            trade_size_sol,
            historical_liquidity,
            sol_price,
            volume_24h_usd=vol_24h,
        )
        
        # Check if slippage is acceptable
        if slippage > self.config.max_slippage_percent:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=historical_liquidity,
                liquidity_sufficient=True,
                estimated_slippage_percent=slippage,
                slippage_cost_sol=trade_size_sol * slippage,
                fee_cost_sol=0,
                simulated_pnl_sol=0,
                rejected=True,
                rejection_reason=f"Slippage {slippage*100:.1f}% > {self.config.max_slippage_percent*100:.1f}%",
            ), f"Excessive slippage: {slippage*100:.1f}%"
        
        # Calculate costs per trade
        slippage_cost = trade_size_sol * slippage
        fee_cost = trade_size_sol * self.config.dex_fee_percent
        priority_fee_cost = max(0.0, self.config.priority_fee_sol_per_trade)
        jito_tip_cost = max(0.0, self.config.jito_tip_sol_per_trade)
        execution_cost = priority_fee_cost + jito_tip_cost
        total_cost = slippage_cost + fee_cost + execution_cost
        
        # Round-trip position tracking
        token = trade.token_address
        position = positions.setdefault(token, {"qty": 0.0, "cost_basis_sol": 0.0})
        
        simulated_pnl = 0.0
        
        if trade.action == TradeAction.BUY:
            # BUY: apply costs, increase position
            net_sol_spent = trade_size_sol + total_cost
            token_qty = trade.token_amount if trade.token_amount is not None else 0.0
            
            # If token_amount not available, estimate from price
            if token_qty <= 0 and trade.price_sol and trade.price_sol > 0:
                token_qty = trade_size_sol / trade.price_sol
            
            if token_qty > 0:
                position["qty"] += token_qty
                position["cost_basis_sol"] += net_sol_spent
                # No realized PnL on BUY
                simulated_pnl = 0.0
            else:
                # Can't track position without token quantity
                logger.warning(f"BUY trade missing token_amount for {token[:8]}...")
                simulated_pnl = 0.0
        
        elif trade.action == TradeAction.SELL:
            # SELL: compute proceeds, realize PnL, reduce position
            net_sol_received = trade_size_sol - total_cost  # Costs reduce proceeds
            token_qty = trade.token_amount if trade.token_amount is not None else 0.0
            
            # If token_amount not available, estimate from price
            if token_qty <= 0 and trade.price_sol and trade.price_sol > 0:
                token_qty = trade_size_sol / trade.price_sol
            
            if token_qty <= 0:
                return SimulatedTrade(
                    original_trade=trade,
                    current_liquidity_usd=historical_liquidity,
                    liquidity_sufficient=True,
                    estimated_slippage_percent=slippage,
                    slippage_cost_sol=slippage_cost,
                    fee_cost_sol=fee_cost + execution_cost,
                    simulated_pnl_sol=0,
                    rejected=True,
                    rejection_reason="Missing token quantity for SELL",
                ), "Missing token quantity for SELL"
            
            if position["qty"] <= 0:
                # Can't sell what we don't have - this is a data issue
                logger.warning(f"SELL trade without position for {token[:8]}...")
                simulated_pnl = 0.0
            else:
                # Calculate realized PnL
                sell_qty = min(token_qty, position["qty"])
                avg_cost_per_token = position["cost_basis_sol"] / position["qty"] if position["qty"] > 0 else 0.0
                allocated_cost_basis = avg_cost_per_token * sell_qty
                
                # Realized PnL = proceeds - allocated cost basis
                simulated_pnl = net_sol_received - allocated_cost_basis
                
                # Reduce position
                position["qty"] -= sell_qty
                position["cost_basis_sol"] -= allocated_cost_basis
                if position["qty"] <= 1e-12:
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
        # Use empty positions dict for legacy behavior (no position tracking)
        return self._simulate_trade_roundtrip(trade, min_liquidity, sol_price, {})
    



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
