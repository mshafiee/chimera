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
from typing import List, Optional, Tuple
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
        Simulate all historical trades for a wallet.
        
        Args:
            wallet_address: Wallet address being validated
            trades: List of historical trades
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
        
        # Check minimum trade requirement
        if len(trades) < self.config.min_trades_required:
            return SimulatedResult(
                wallet_address=wallet_address,
                total_trades=len(trades),
                simulated_trades=0,
                rejected_trades=0,
                original_pnl_sol=0.0,
                simulated_pnl_sol=0.0,
                pnl_difference_sol=0.0,
                total_slippage_cost_sol=0.0,
                total_fee_cost_sol=0.0,
                passed=False,
                failure_reason=f"Insufficient trades: {len(trades)} < {self.config.min_trades_required}",
            )
        
        # Get minimum liquidity threshold for strategy
        min_liquidity = self.config.get_min_liquidity(strategy)
        sol_price = self.liquidity.get_sol_price_usd()
        
        # Simulate each trade
        simulated_trades: List[SimulatedTrade] = []
        rejected_details: List[str] = []
        
        total_original_pnl = 0.0
        total_simulated_pnl = 0.0
        total_slippage = 0.0
        total_fees = 0.0
        rejected_count = 0
        
        for trade in trades:
            sim_trade, rejection_reason = self._simulate_trade(
                trade, min_liquidity, sol_price
            )
            simulated_trades.append(sim_trade)
            
            # Track original PnL
            if trade.pnl_sol is not None:
                total_original_pnl += trade.pnl_sol
            
            if sim_trade.rejected:
                rejected_count += 1
                rejected_details.append(
                    f"{trade.token_symbol}: {rejection_reason}"
                )
            else:
                # Add simulated PnL (minus costs)
                total_simulated_pnl += sim_trade.simulated_pnl_sol
                total_slippage += sim_trade.slippage_cost_sol
                total_fees += sim_trade.fee_cost_sol
        
        # Calculate rejection rate
        rejection_rate = rejected_count / len(trades)
        
        # Determine pass/fail
        passed = True
        failure_reason = None
        
        # Fail if too many trades rejected (>50%)
        if rejection_rate > 0.5:
            passed = False
            failure_reason = f"Too many trades rejected: {rejection_rate*100:.0f}%"
        
        # Fail if simulated PnL is negative
        elif total_simulated_pnl < 0:
            passed = False
            failure_reason = f"Negative simulated PnL: {total_simulated_pnl:.4f} SOL"
        
        # Fail if PnL reduction is too high (>80% reduction)
        elif total_original_pnl > 0:
            pnl_reduction = (total_original_pnl - total_simulated_pnl) / total_original_pnl
            if pnl_reduction > 0.8:
                passed = False
                failure_reason = f"PnL reduction too high: {pnl_reduction*100:.0f}%"
        
        return SimulatedResult(
            wallet_address=wallet_address,
            total_trades=len(trades),
            simulated_trades=len(trades) - rejected_count,
            rejected_trades=rejected_count,
            original_pnl_sol=total_original_pnl,
            simulated_pnl_sol=total_simulated_pnl,
            pnl_difference_sol=total_original_pnl - total_simulated_pnl,
            total_slippage_cost_sol=total_slippage,
            total_fee_cost_sol=total_fees,
            rejected_trade_details=rejected_details,
            passed=passed,
            failure_reason=failure_reason,
        )
    
    def _simulate_trade(
        self,
        trade: HistoricalTrade,
        min_liquidity: float,
        sol_price: float,
    ) -> Tuple[SimulatedTrade, Optional[str]]:
        """
        Simulate a single trade under historical market conditions.
        
        Uses historical liquidity at the time of the trade, falling back
        to current liquidity if historical data is unavailable.
        
        Args:
            trade: Historical trade to simulate
            min_liquidity: Minimum liquidity requirement (USD)
            sol_price: Current SOL price in USD
            
        Returns:
            Tuple of (SimulatedTrade, rejection_reason)
        """
        # Get historical liquidity for the token at trade timestamp
        # Falls back to current liquidity if historical unavailable
        liquidity_data = self.liquidity.get_historical_liquidity_or_current(
            trade.token_address,
            trade.timestamp,
        )
        
        # Collect current liquidity snapshot for future historical queries
        # (if we don't have historical data, store current as historical)
        if liquidity_data and liquidity_data.source.endswith("_fallback"):
            # We used current liquidity as fallback, store it with historical timestamp
            current_liq = self.liquidity.get_current_liquidity(trade.token_address)
            if current_liq:
                historical_snapshot = LiquidityData(
                    token_address=current_liq.token_address,
                    liquidity_usd=current_liq.liquidity_usd,
                    price_usd=current_liq.price_usd,
                    volume_24h_usd=current_liq.volume_24h_usd,
                    timestamp=trade.timestamp,  # Use trade timestamp
                    source="backtester_collection",
                )
                self.liquidity._store_in_database(historical_snapshot)
        
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
        
        current_liquidity = liquidity_data.liquidity_usd
        
        # Check liquidity requirement
        if current_liquidity < min_liquidity:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=current_liquidity,
                liquidity_sufficient=False,
                estimated_slippage_percent=1.0,
                slippage_cost_sol=trade.amount_sol,
                fee_cost_sol=0,
                simulated_pnl_sol=0,
                rejected=True,
                rejection_reason=f"Liquidity ${current_liquidity:,.0f} < ${min_liquidity:,.0f}",
            ), f"Insufficient liquidity: ${current_liquidity:,.0f}"
        
        # Estimate slippage
        slippage = self.liquidity.estimate_slippage(
            trade.token_address,
            trade.amount_sol,
            current_liquidity,
            sol_price,
        )
        
        # Check if slippage is acceptable
        if slippage > self.config.max_slippage_percent:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=current_liquidity,
                liquidity_sufficient=True,
                estimated_slippage_percent=slippage,
                slippage_cost_sol=trade.amount_sol * slippage,
                fee_cost_sol=0,
                simulated_pnl_sol=0,
                rejected=True,
                rejection_reason=f"Slippage {slippage*100:.1f}% > {self.config.max_slippage_percent*100:.1f}%",
            ), f"Excessive slippage: {slippage*100:.1f}%"
        
        # Calculate costs
        slippage_cost = trade.amount_sol * slippage
        fee_cost = trade.amount_sol * self.config.dex_fee_percent
        
        # Calculate simulated PnL
        # For BUY trades: original PnL minus costs
        # For SELL trades: original PnL minus costs
        original_pnl = trade.pnl_sol or 0.0
        simulated_pnl = original_pnl - slippage_cost - fee_cost
        
        return SimulatedTrade(
            original_trade=trade,
            current_liquidity_usd=current_liquidity,
            liquidity_sufficient=True,
            estimated_slippage_percent=slippage,
            slippage_cost_sol=slippage_cost,
            fee_cost_sol=fee_cost,
            simulated_pnl_sol=simulated_pnl,
            rejected=False,
            rejection_reason=None,
        ), None
    
    def estimate_promotion_viability(
        self,
        trades: List[HistoricalTrade],
        strategy: str = "SHIELD",
    ) -> dict:
        """
        Quick estimate of whether a wallet would pass backtest.
        
        This is a lighter-weight check that can be run before full simulation.
        
        Args:
            trades: Historical trades
            strategy: Strategy type
            
        Returns:
            Dict with viability assessment
        """
        if len(trades) < self.config.min_trades_required:
            return {
                "viable": False,
                "reason": "Insufficient trades",
                "trade_count": len(trades),
                "required": self.config.min_trades_required,
            }
        
        min_liquidity = self.config.get_min_liquidity(strategy)
        
        # Quick liquidity check on unique tokens
        unique_tokens = set(t.token_address for t in trades)
        low_liquidity_tokens = []
        
        for token in unique_tokens:
            liq = self.liquidity.get_current_liquidity(token)
            if liq and liq.liquidity_usd < min_liquidity:
                low_liquidity_tokens.append(token[:8] + "...")
        
        if len(low_liquidity_tokens) > len(unique_tokens) * 0.5:
            return {
                "viable": False,
                "reason": "Too many low-liquidity tokens",
                "low_liquidity_count": len(low_liquidity_tokens),
                "total_tokens": len(unique_tokens),
            }
        
        return {
            "viable": True,
            "reason": "Passes preliminary checks",
            "trade_count": len(trades),
            "unique_tokens": len(unique_tokens),
            "low_liquidity_tokens": len(low_liquidity_tokens),
        }


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
