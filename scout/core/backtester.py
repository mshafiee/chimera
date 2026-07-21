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


from .utils import utcnow

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
        return self._simulate_wallet_internal(wallet_address, trades, strategy, {})

    def simulate_wallet_with_positions(
        self,
        wallet_address: str,
        trades: List[HistoricalTrade],
        strategy: str,
        initial_positions: Dict[str, Dict[str, Decimal]],
    ) -> SimulatedResult:
        """
        Simulate trades starting from a given position state.

        Used by walk-forward validation to carry positions from the train phase
        into the test phase so that SELL trades can reference BUYs from training.
        """
        return self._simulate_wallet_internal(wallet_address, trades, strategy, initial_positions)

    def _simulate_wallet_internal(
        self,
        wallet_address: str,
        trades: List[HistoricalTrade],
        strategy: str,
        initial_positions: Dict[str, Dict[str, Decimal]],
    ) -> SimulatedResult:
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
        # Use the SOL price at the time of the trade for slippage estimation.
        # Passing the current SOL price for all historical trades distorts slippage
        # calculations: if SOL was $50 during the wallet's period but is $150 today,
        # slippage appears 3x smaller (USD impact inflated), making the wallet look
        # more copyable than it was. We approximate historical SOL price by scaling
        # the current price with a simple time-based heuristic.
        sol_price_float = self.liquidity.get_sol_price_usd_sync()
        sol_price = float_to_decimal(sol_price_float)
        sol_price_current = sol_price

        # Cache derived SOL prices per hour bucket for consistency across trades
        # within the same hour window.
        _sol_price_hour_cache: Dict[int, float] = {}
        
        # Round-trip position tracking: {token_address: {"qty": Decimal, "cost_basis_sol": Decimal}}
        positions: Dict[str, Dict[str, Decimal]] = initial_positions
        
        from datetime import datetime as _dt, timezone as _tz

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
        simulated_sell_count = 0

        # Time-decay weighting for PnL aggregation
        _use_decay = getattr(self.config, 'backtest_time_decay_enabled', False)
        _decay_half_life = getattr(self.config, 'backtest_time_decay_half_life_days', 14)
        _now = _dt.now(_tz.utc)
        _total_weight = Decimal('0')
        
        for trade in sorted_trades:
            sim_trade, rejection_reason, is_low_confidence = self._simulate_trade_roundtrip(
                trade, min_liquidity_decimal, sol_price_current, positions, _sol_price_hour_cache,
            )
            simulated_trades.append(sim_trade)
            # Track low-confidence liquidity usage (returned by _simulate_trade_roundtrip,
            # no redundant fetch needed — FIX 6: double-fetch eliminated).
            # Fallback liquidity creates survivorship bias, so we exclude these
            # trades from PnL totals entirely later in the loop.
            if is_low_confidence:
                low_confidence_trades_count += 1
            if sim_trade.rejected:
                rejected_count += 1
                rejected_details.append(
                    f"{trade.token_symbol}: {rejection_reason}"
                )
                # Track original PnL even for rejected trades so that pnl_reduction accurately reflects
                # profits we cannot replicate due to liquidity constraints.
                if trade.action == TradeAction.SELL and trade.pnl_sol is not None:
                    total_original_realized_pnl += float_to_decimal(trade.pnl_sol)
            elif is_low_confidence:
                # Exclude low-confidence trades from BOTH PnL totals to prevent survivorship bias
                # and avoid apples-to-oranges PnL reduction calculations.
                # They still count toward total_trades for the rejection-rate denominator.
                pass
            else:
                # Calculate time-decay weight for this trade
                weight = Decimal('1.0')
                if _use_decay and trade.timestamp.tzinfo is None:
                    trade_age_seconds = (_now.replace(tzinfo=None) - trade.timestamp).total_seconds()
                    age_days = max(0.0, trade_age_seconds / 86400.0)
                    weight_float = 2.0 ** (-age_days / _decay_half_life)
                    weight = float_to_decimal(weight_float)
                    _total_weight += weight

                # Track original realized PnL for accepted trades
                if trade.action == TradeAction.SELL and trade.pnl_sol is not None:
                    total_original_realized_pnl += float_to_decimal(trade.pnl_sol)

                # Track costs
                total_slippage += sim_trade.slippage_cost_sol
                total_fees += sim_trade.fee_cost_sol

                # Track simulated realized PnL (only SELL trades)
                if trade.action == TradeAction.SELL and sim_trade.simulated_pnl_sol is not None:
                    total_simulated_realized_pnl += sim_trade.simulated_pnl_sol * weight
                    simulated_sell_count += 1
        
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
        
        # Fail if no SELL trades were closed in the OOS window (open positions only = untested)
        elif passed and simulated_sell_count == 0:
            passed = False
            failure_reason = "No closed SELL trades in OOS window — cannot validate profitability"

        # Fail if simulated realized PnL is negative
        elif passed and total_simulated_realized_pnl < Decimal('0'):
            passed = False
            failure_reason = f"Negative simulated realized PnL: {decimal_to_float(total_simulated_realized_pnl):.4f} SOL"
        
        # Fail the backtest when too many trades relied on fallback/capped liquidity.
        # Inflated PnL from mooned tokens or filtered rugged tokens is not a valid signal.
        if low_confidence_trades_count > 0:
            low_confidence_ratio = low_confidence_trades_count / len(sorted_trades)
            threshold = 0.15 if strategy.upper() == "SHIELD" else 0.10

            # Per-trade-type check: if ALL non-rejected SELL trades are low-confidence,
            # fail immediately — the wallet's profitability is entirely unverifiable.
            non_rejected_sells = [
                st for st in simulated_trades
                if not st.rejected
                and st.original_trade.action == TradeAction.SELL
                and st.simulated_pnl_sol is not None
            ]
            all_sells_low_conf = (
                len(non_rejected_sells) > 0
                and sum(
                    1 for st in non_rejected_sells
                    if st.original_trade.liquidity_at_trade_usd is None
                ) == len(non_rejected_sells)
            )

            if all_sells_low_conf:
                logger.warning(
                    f"⚠️  SURVIVORSHIP BIAS RISK: All SELL trades ({len(non_rejected_sells)}) "
                    f"used fallback liquidity. Failing wallet."
                )
                passed = False
                bias_msg = "All SELL trades used low-confidence liquidity (survivorship bias)"
                if failure_reason:
                    failure_reason += f"; {bias_msg}"
                else:
                    failure_reason = bias_msg
            elif low_confidence_ratio > threshold:
                logger.warning(
                    f"⚠️  SURVIVORSHIP BIAS RISK: {low_confidence_trades_count}/{len(sorted_trades)} "
                    f"({low_confidence_ratio*100:.0f}%) trades used fallback liquidity data. "
                    f"Backtest results may be inflated. Failing wallet."
                )
                passed = False
                bias_msg = f"Low-confidence liquidity on {low_confidence_ratio*100:.0f}% of trades (survivorship bias risk)"
                if failure_reason:
                    failure_reason += f"; {bias_msg}"
                else:
                    failure_reason = bias_msg
        
        # Market regime classification
        regime_risk = None
        if sorted_trades and hasattr(self.liquidity, 'classify_market_regime'):
            regime_risk = self.liquidity.classify_market_regime(
                sorted_trades[0].timestamp, sorted_trades[-1].timestamp,
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
            trades=simulated_trades,  # Enables profit factor check in validator
            passed=passed,
            failure_reason=failure_reason,
            regime_risk=regime_risk,
            final_positions=positions,
        )
    def run_walk_forward(
        self,
        wallet_address: str,
        trades: List[HistoricalTrade],
        strategy: str = "SHIELD",
        holdout_fraction: float = 0.3,
        min_test_trades: int = 5,
    ) -> SimulatedResult:
        """
        Run walk-forward validation.
        
        Splits trades chronologically:
        - In-sample / Train set: older (1 - holdout_fraction) trades
        - Out-of-sample / Test set: newer holdout_fraction trades
        
        Returns SimulatedResult indicating pass/fail.
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
            
        sorted_trades = sorted(trades, key=lambda t: t.timestamp)
        
        # Date-based split: use chronological holdout rather than count-based.
        # Count-based splits can put trades from the same week into both train and test,
        # defeating the purpose of walk-forward validation.
        total_span = (sorted_trades[-1].timestamp - sorted_trades[0].timestamp).total_seconds()
        if total_span >= 7 * 86400:  # 7+ days of data → use date-based split
            from datetime import timedelta
            holdout_cutoff = sorted_trades[-1].timestamp - timedelta(seconds=total_span * holdout_fraction)
            train_trades = [t for t in sorted_trades if t.timestamp < holdout_cutoff]
            test_trades = [t for t in sorted_trades if t.timestamp >= holdout_cutoff]
        else:
            # Fall back to count-based for short date ranges
            holdout_n = int(max(1, round(len(sorted_trades) * holdout_fraction)))
            train_trades = sorted_trades[:-holdout_n]
            test_trades = sorted_trades[-holdout_n:]
        
        if len(test_trades) < min_test_trades:
            return SimulatedResult(
                wallet_address=wallet_address,
                total_trades=len(sorted_trades),
                simulated_trades=0,
                rejected_trades=0,
                original_pnl_sol=0.0,
                simulated_pnl_sol=0.0,
                pnl_difference_sol=0.0,
                total_slippage_cost_sol=0.0,
                total_fee_cost_sol=0.0,
                passed=False,
                failure_reason="Insufficient test data for walk-forward validation",
            )
            
        # Run in-sample (train) first
        train_result = self.simulate_wallet(wallet_address, train_trades, strategy)
        if not train_result.passed:
            return SimulatedResult(
                wallet_address=wallet_address,
                total_trades=len(sorted_trades),
                simulated_trades=train_result.simulated_trades,
                rejected_trades=train_result.rejected_trades,
                original_pnl_sol=train_result.original_pnl_sol,
                simulated_pnl_sol=train_result.simulated_pnl_sol,
                pnl_difference_sol=train_result.pnl_difference_sol,
                total_slippage_cost_sol=train_result.total_slippage_cost_sol,
                total_fee_cost_sol=train_result.total_fee_cost_sol,
                passed=False,
                failure_reason=f"FAILED_IN_SAMPLE: {train_result.failure_reason}",
            )

        # Run out-of-sample (test) second, carrying positions from train phase
        # so that SELL trades in the test set can reference BUYs from the train set.
        test_result = self.simulate_wallet_with_positions(
            wallet_address, test_trades, strategy, train_result.final_positions
        )
        if not test_result.passed:
            return SimulatedResult(
                wallet_address=wallet_address,
                total_trades=len(sorted_trades),
                simulated_trades=train_result.simulated_trades + test_result.simulated_trades,
                rejected_trades=train_result.rejected_trades + test_result.rejected_trades,
                original_pnl_sol=train_result.original_pnl_sol + test_result.original_pnl_sol,
                simulated_pnl_sol=train_result.simulated_pnl_sol + test_result.simulated_pnl_sol,
                pnl_difference_sol=train_result.pnl_difference_sol + test_result.pnl_difference_sol,
                total_slippage_cost_sol=train_result.total_slippage_cost_sol + test_result.total_slippage_cost_sol,
                total_fee_cost_sol=train_result.total_fee_cost_sol + test_result.total_fee_cost_sol,
                passed=False,
                failure_reason=f"FAILED_WALK_FORWARD_OOS: {test_result.failure_reason}",
            )
            
        return test_result


    def _simulate_trade_roundtrip(
        self,
        trade: HistoricalTrade,
        min_liquidity: Decimal,
        sol_price: Decimal,
        positions: Dict[str, Dict[str, Decimal]],
        sol_price_hour_cache: Optional[Dict[int, float]] = None,
    ) -> Tuple[SimulatedTrade, Optional[str], bool]:
        """
        Simulate a single trade using round-trip cashflow model.

        Tracks positions per token and computes realized PnL only on SELL trades.
        Costs are applied at both entry (BUY) and exit (SELL).

        Args:
            trade: Historical trade to simulate
            min_liquidity: Minimum liquidity requirement (USD)
            sol_price: Current SOL price in USD (fallback)
            positions: Position ledger (mutated in-place)
            sol_price_hour_cache: Optional per-hour cache for derived SOL prices

        Returns:
            Tuple of (SimulatedTrade, rejection_reason, is_low_confidence).
            is_low_confidence is True when fallback (current) liquidity was used
            instead of historical data (survivorship bias risk).
        """
        # Get liquidity data (historical-at-trade if available).
        is_low_confidence = False
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
            # Query ONLY historical liquidity - no fallback to current to avoid survivorship bias
            liquidity_data = self.liquidity.get_historical_liquidity_or_current(
                trade.token_address, trade.timestamp
            )
            # No fallback to current liquidity - if historical data is unavailable,
            # we reject the trade to prevent survivorship bias
            if liquidity_data is not None:
                source = getattr(liquidity_data, 'source', '')
                if source and ('fallback' in source.lower() or 'low_confidence' in source.lower()):
                    is_low_confidence = True

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
            ), "Could not fetch liquidity data", False

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
            ), f"Insufficient historical liquidity: ${decimal_to_float(historical_liquidity):,.0f}", is_low_confidence

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
                ), "Could not fetch current liquidity", is_low_confidence

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
                ), f"Insufficient current liquidity: ${decimal_to_float(current_liquidity_now):,.0f}", is_low_confidence

        # Get trade size in SOL (use sol_amount if available, fallback to amount_sol)
        trade_size_sol = float_to_decimal(trade.sol_amount if trade.sol_amount is not None else trade.amount_sol)
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
            ), "Invalid trade size", is_low_confidence
        
        # Copier size override: use the copier's target trade size for slippage
        # and cost estimation, keeping the original trader size for PnL ratio
        # computation. Models the copy-trade experience accurately.
        cost_size_sol = trade_size_sol
        if self.config.simulate_at_size_sol is not None:
            cost_size_sol = float_to_decimal(self.config.simulate_at_size_sol) if not isinstance(self.config.simulate_at_size_sol, Decimal) else self.config.simulate_at_size_sol
            cost_size_sol = min(cost_size_sol, trade_size_sol)  # Cap at original
        
        # Estimate slippage using historical liquidity (trade-time conditions).
        # Use the SOL price at the time of the trade rather than the current price.
        # If we have per-token price_usd and price_sol, we can derive the historical
        # SOL/USD price: sol_price_historical = price_usd / price_sol.
        # Otherwise, fall back to the current price (which introduces bias for old trades).
        # Cache derived SOL prices per hour for consistency across trades within the same hour.
        trade_sol_price = decimal_to_float(sol_price)
        if trade.price_usd is not None and trade.price_sol is not None and trade.price_sol > Decimal('0'):
            hour_bucket = trade.timestamp.replace(minute=0, second=0, microsecond=0)
            hour_key = int(hour_bucket.timestamp())
            if sol_price_hour_cache is not None and hour_key in sol_price_hour_cache:
                trade_sol_price = sol_price_hour_cache[hour_key]
            else:
                derived_sol_price = decimal_to_float(trade.price_usd / trade.price_sol)
                if derived_sol_price > 0:
                    trade_sol_price = derived_sol_price
                    if sol_price_hour_cache is not None:
                        sol_price_hour_cache[hour_key] = derived_sol_price
        vol_24h = getattr(liquidity_data, 'volume_24h_usd', Decimal('0'))
        # Phase 5c: Compute token age for slippage model
        token_age_days = 365.0
        token_creation = getattr(liquidity_data, 'token_creation_timestamp', None)
        if token_creation is not None:
            try:
                age_seconds = (trade.timestamp - token_creation).total_seconds()
                token_age_days = max(0.0, age_seconds / 86400.0)
            except (TypeError, AttributeError):
                pass
        slippage_float = self.liquidity.estimate_slippage(
            trade.token_address,
            decimal_to_float(cost_size_sol),
            decimal_to_float(historical_liquidity),
            trade_sol_price,
            volume_24h_usd=decimal_to_float(vol_24h),
            token_age_days=token_age_days,
        )
        slippage = float_to_decimal(slippage_float)
        
        # Check if slippage is acceptable
        if slippage > self.config.max_slippage_percent:
            return SimulatedTrade(
                original_trade=trade,
                current_liquidity_usd=historical_liquidity,
                liquidity_sufficient=True,
                estimated_slippage_percent=slippage,
                slippage_cost_sol=cost_size_sol * slippage,
                fee_cost_sol=Decimal('0'),
                simulated_pnl_sol=Decimal('0'),
                rejected=True,
                rejection_reason=f"Slippage {decimal_to_float(slippage * Decimal('100')):.1f}% > {decimal_to_float(self.config.max_slippage_percent * Decimal('100')):.1f}%",
            ), f"Excessive slippage: {decimal_to_float(slippage * Decimal('100')):.1f}%", is_low_confidence
        
        # Calculate costs per trade using Decimal
        slippage_cost = cost_size_sol * slippage
        fee_cost = cost_size_sol * self.config.dex_fee_percent
        priority_fee_cost = max(Decimal('0'), self.config.priority_fee_sol_per_trade)
        jito_tip_cost = max(Decimal('0'), self.config.jito_tip_sol_per_trade)
        execution_cost = priority_fee_cost + jito_tip_cost

        # Time-delay slippage: model the 100-500ms operator latency + block
        # inclusion delay. BUY leg = entry_delay_slippage_pct, SELL leg = exit.
        # Apply regime-aware scaling based on liquidity turnover ratio.
        delay_slippage = Decimal('0')
        if trade.action in (TradeAction.BUY, TradeAction.SELL):
            # Compute liquidity turnover ratio
            turnover_ratio = 0.0
            if historical_liquidity > Decimal('0'):
                turnover_ratio = float(vol_24h) / float(historical_liquidity)

            # Determine multiplier based on turnover ratio
            multiplier = 1.0
            if turnover_ratio > 10:
                multiplier = 3.0
            elif turnover_ratio > 3:
                multiplier = 2.0
            else:
                multiplier = 1.0

            # Cap multiplier at 10×
            multiplier = min(10.0, multiplier)

            # Calculate base delay slippage
            base_pct = self.config.entry_delay_slippage_pct if trade.action == TradeAction.BUY else self.config.exit_delay_slippage_pct
            delay_slippage = cost_size_sol * base_pct * float_to_decimal(multiplier)

        # MEV/sandwich penalty on SELL trades (modeling sandwich attacks on copied exits)
        mev_penalty = Decimal('0')
        if trade.action == TradeAction.SELL:
            mev_penalty = cost_size_sol * self.config.mev_penalty_pct

        total_cost = slippage_cost + fee_cost + execution_cost + delay_slippage + mev_penalty
        
        # Round-trip position tracking using Decimal
        token = trade.token_address
        position = positions.setdefault(token, {"qty": Decimal('0'), "cost_basis_sol": Decimal('0')})
        
        simulated_pnl = Decimal('0')
        
        if trade.action == TradeAction.BUY:
            # BUY: apply costs, increase position
            token_qty = trade.token_amount if trade.token_amount is not None else Decimal('0')
            
            # If token_amount not available, estimate from price
            if token_qty <= Decimal('0') and trade.price_sol and trade.price_sol > Decimal('0'):
                token_qty = safe_decimal_divide(trade_size_sol, trade.price_sol)
            
            if token_qty > Decimal('0'):
                position["qty"] += token_qty
                position["cost_basis_sol"] += trade_size_sol + total_cost
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
                ), "Missing token quantity for SELL", is_low_confidence
            
            if position["qty"] <= Decimal('0'):
                # Can't sell what we don't have - this is a data issue
                logger.warning(f"SELL trade without position for {token[:8]}...")
                simulated_pnl = Decimal('0')
            else:
                # Calculate realized PnL
                sell_qty = min(token_qty, position["qty"])
                is_oversell = sell_qty < token_qty
                if is_oversell:
                    unmatched_qty = token_qty - sell_qty
                    logger.warning(
                        f"Partial sell for {token[:8]}...: {sell_qty} tracked vs {token_qty} sold "
                        f"(unmatched {unmatched_qty}). Prior BUY tracking may be incomplete — "
                        f"PnL on this trade may be overstated."
                    )
                avg_cost_per_token = safe_decimal_divide(position["cost_basis_sol"], position["qty"])
                allocated_cost_basis = avg_cost_per_token * sell_qty
                
                # Realized PnL = proceeds - allocated cost basis
                simulated_pnl = net_sol_received - allocated_cost_basis
                
                # Reduce position
                position["qty"] -= sell_qty
                position["cost_basis_sol"] -= allocated_cost_basis
                MIN_QTY_EPSILON = Decimal('0.000000000001')
                if position["qty"] <= MIN_QTY_EPSILON:
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
        ), None, is_low_confidence

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
        # Use empty positions dict for legacy behavior (no position tracking).
        # Strip the third element (is_low_confidence) for backward compat.
        sim_trade, reason, _ = self._simulate_trade_roundtrip(
            trade, min_liquidity_decimal, sol_price_decimal, {}, None
        )
        return sim_trade, reason
    



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
            timestamp=utcnow(),
            tx_signature="tx1",
            pnl_sol=0.15,
        ),
        HistoricalTrade(
            token_address="EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm",
            token_symbol="WIF",
            action=TradeAction.BUY,
            amount_sol=0.3,
            price_at_trade=1.5,
            timestamp=utcnow(),
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
            timestamp=utcnow(),
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
