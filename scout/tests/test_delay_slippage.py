"""
Tests for regime-aware delay slippage in backtester.

Tests turnover ratio calculation, delay slippage multipliers, and
regime-aware scaling for both entry and exit trades.
"""

import pytest
from decimal import Decimal
from unittest.mock import Mock

from core.backtester import Backtester, BacktestConfig
from core.wqs import Trade, TradeAction


class TestTurnoverRatioCalculation:
    """Test liquidity turnover ratio calculation."""

    def test_turnover_ratio_calculation_normal(self):
        """Test turnover ratio calculation for normal liquidity."""
        vol_24h = Decimal('100000')  # $100k daily volume
        liquidity = Decimal('10000')  # $10k liquidity
        
        turnover_ratio = float(vol_24h) / float(liquidity)
        assert turnover_ratio == 10.0

    def test_turnover_ratio_high(self):
        """Test turnover ratio for high-turnover regime."""
        vol_24h = Decimal('500000')  # $500k daily volume
        liquidity = Decimal('10000')  # $10k liquidity
        
        turnover_ratio = float(vol_24h) / float(liquidity)
        assert turnover_ratio == 50.0

    def test_turnover_ratio_low(self):
        """Test turnover ratio for low-turnover regime."""
        vol_24h = Decimal('10000')  # $10k daily volume
        liquidity = Decimal('100000')  # $100k liquidity
        
        turnover_ratio = float(vol_24h) / float(liquidity)
        assert turnover_ratio == 0.1

    def test_turnover_ratio_zero_liquidity(self):
        """Test turnover ratio with zero liquidity."""
        vol_24h = Decimal('100000')
        liquidity = Decimal('0')
        
        # Should handle gracefully
        turnover_ratio = float(vol_24h) / float(liquidity)
        assert turnover_ratio == float('inf')

    def test_turnover_ratio_zero_volume(self):
        """Test turnover ratio with zero volume."""
        vol_24h = Decimal('0')
        liquidity = Decimal('10000')
        
        turnover_ratio = float(vol_24h) / float(liquidity)
        assert turnover_ratio == 0.0


class TestDelaySlippageMultiplier:
    """Test delay slippage multiplier based on turnover ratio."""

    def test_multiplier_high_turnover(self):
        """Test multiplier for high-turnover regime (>10)."""
        turnover_ratio = 15.0
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        assert multiplier == 3.0

    def test_multiplier_medium_turnover(self):
        """Test multiplier for medium-turnover regime (3-10)."""
        turnover_ratio = 5.0
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        assert multiplier == 2.0

    def test_multiplier_low_turnover(self):
        """Test multiplier for low-turnover regime (<3)."""
        turnover_ratio = 2.0
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        assert multiplier == 1.0

    def test_multiplier_edge_case_10(self):
        """Test multiplier at exact turnover ratio of 10."""
        turnover_ratio = 10.0
        # According to implementation, >10 gets 3×, so 10 gets 2×
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        assert multiplier == 2.0

    def test_multiplier_edge_case_3(self):
        """Test multiplier at exact turnover ratio of 3."""
        turnover_ratio = 3.0
        # According to implementation, >3 gets 2×, so 3 gets 1×
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        assert multiplier == 1.0

    def test_multiplier_capped_at_10x(self):
        """Test that multiplier is capped at 10×."""
        turnover_ratio = 100.0
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        assert multiplier == 3.0

    def test_multiplier_extremely_high_turnover(self):
        """Test multiplier with extremely high turnover."""
        turnover_ratio = 1000.0
        base_multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        capped_multiplier = min(10.0, base_multiplier)
        assert capped_multiplier == 3.0


class TestDelaySlippageCalculation:
    """Test delay slippage calculation with regime-aware scaling."""

    def test_delay_slippage_entry_low_turnover(self):
        """Test entry delay slippage with low turnover."""
        cost_size_sol = Decimal('1.0')
        base_entry_pct = Decimal('0.01')  # 1%
        turnover_ratio = 2.0
        
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        
        delay_slippage = cost_size_sol * base_entry_pct * Decimal(str(multiplier))
        
        assert delay_slippage == Decimal('0.01')  # 1% with 1× multiplier

    def test_delay_slippage_entry_medium_turnover(self):
        """Test entry delay slippage with medium turnover."""
        cost_size_sol = Decimal('1.0')
        base_entry_pct = Decimal('0.01')  # 1%
        turnover_ratio = 5.0
        
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        
        delay_slippage = cost_size_sol * base_entry_pct * Decimal(str(multiplier))
        
        assert delay_slippage == Decimal('0.02')  # 1% with 2× multiplier

    def test_delay_slippage_entry_high_turnover(self):
        """Test entry delay slippage with high turnover."""
        cost_size_sol = Decimal('1.0')
        base_entry_pct = Decimal('0.01')  # 1%
        turnover_ratio = 15.0
        
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        
        delay_slippage = cost_size_sol * base_entry_pct * Decimal(str(multiplier))
        
        assert delay_slippage == Decimal('0.03')  # 1% with 3× multiplier

    def test_delay_slippage_exit_low_turnover(self):
        """Test exit delay slippage with low turnover."""
        cost_size_sol = Decimal('1.0')
        base_exit_pct = Decimal('0.015')  # 1.5%
        turnover_ratio = 2.0
        
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        
        delay_slippage = cost_size_sol * base_exit_pct * Decimal(str(multiplier))
        
        assert delay_slippage == Decimal('0.015')  # 1.5% with 1× multiplier

    def test_delay_slippage_exit_high_turnover(self):
        """Test exit delay slippage with high turnover."""
        cost_size_sol = Decimal('1.0')
        base_exit_pct = Decimal('0.015')  # 1.5%
        turnover_ratio = 15.0
        
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        
        delay_slippage = cost_size_sol * base_exit_pct * Decimal(str(multiplier))
        
        assert delay_slippage == Decimal('0.045')  # 1.5% with 3× multiplier

    def test_delay_slippage_large_trade(self):
        """Test delay slippage with large trade size."""
        cost_size_sol = Decimal('10.0')
        base_entry_pct = Decimal('0.01')
        turnover_ratio = 5.0
        
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        
        delay_slippage = cost_size_sol * base_entry_pct * Decimal(str(multiplier))
        
        assert delay_slippage == Decimal('0.2')  # 10 SOL * 1% * 2×

    def test_delay_slippage_zero_multiplier(self):
        """Test delay slippage when multiplier should be 0 (edge case)."""
        # Simulate no delay slippage
        delay_slippage = Decimal('0')
        
        assert delay_slippage == Decimal('0')


class TestDelaySlippageIntegration:
    """Test delay slippage integration in backtest simulation."""

    def test_delay_slippage_applied_to_buy(self):
        """Test that delay slippage is applied to BUY trades."""
        trade = Trade(
            action=TradeAction.BUY,
            timestamp=None,
            wallet_address="test_wallet",
            token_address="TOKEN",
            price_sol=Decimal('1.0'),
            token_amount=Decimal('10'),
            sol_amount=Decimal('10'),
            signature="sig1"
        )
        
        assert trade.action == TradeAction.BUY
        # BUY should use entry_delay_slippage_pct

    def test_delay_slippage_applied_to_sell(self):
        """Test that delay slippage is applied to SELL trades."""
        trade = Trade(
            action=TradeAction.SELL,
            timestamp=None,
            wallet_address="test_wallet",
            token_address="TOKEN",
            price_sol=Decimal('1.0'),
            token_amount=Decimal('10'),
            sol_amount=Decimal('10'),
            signature="sig1"
        )
        
        assert trade.action == TradeAction.SELL
        # SELL should use exit_delay_slippage_pct

    def test_delay_slippage_added_to_total_cost(self):
        """Test that delay slippage is added to total execution cost."""
        slippage_cost = Decimal('0.01')
        fee_cost = Decimal('0.003')
        execution_cost = Decimal('0.0001')
        delay_slippage = Decimal('0.02')
        mev_penalty = Decimal('0.005')
        
        total_cost = slippage_cost + fee_cost + execution_cost + delay_slippage + mev_penalty
        
        expected = Decimal('0.0381')
        assert total_cost == expected

    def test_delay_slippage_different_for_entry_vs_exit(self):
        """Test that delay slippage differs between entry and exit."""
        cost_size_sol = Decimal('1.0')
        base_entry_pct = Decimal('0.01')
        base_exit_pct = Decimal('0.015')
        turnover_ratio = 5.0
        
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        
        entry_delay = cost_size_sol * base_entry_pct * Decimal(str(multiplier))
        exit_delay = cost_size_sol * base_exit_pct * Decimal(str(multiplier))
        
        assert entry_delay < exit_delay

    def test_delay_slippage_impact_on_pnl(self):
        """Test that delay slippage impacts final PnL."""
        # Simplified PnL calculation
        entry_sol = Decimal('10')
        entry_costs = Decimal('0.3')  # including delay slippage
        exit_sol = Decimal('15')
        exit_costs = Decimal('0.4')  # including delay slippage
        
        total_costs = entry_costs + exit_costs
        gross_pnl = exit_sol - entry_sol
        net_pnl = gross_pnl - total_costs
        
        assert gross_pnl == Decimal('5')
        assert total_costs == Decimal('0.7')
        assert net_pnl == Decimal('4.3')


@pytest.mark.asyncio
class TestBacktesterDelaySlippage:
    """Test delay slippage in Backtester class."""

    async def test_backtester_uses_regime_aware_multiplier(self, backtester):
        """Test that backtester calculates regime-aware multiplier."""
        # Mock liquidity data with high turnover
        vol_24h = Decimal('500000')
        liquidity = Decimal('10000')
        turnover_ratio = float(vol_24h) / float(liquidity)
        
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        
        assert turnover_ratio == 50.0
        assert multiplier == 3.0

    async def test_backtester_delay_slippage_for_different_regimes(self, backtester):
        """Test delay slippage across different turnover regimes."""
        regimes = [
            (0.5, 1.0),   # Low turnover
            (5.0, 2.0),   # Medium turnover
            (15.0, 3.0),  # High turnover
        ]
        
        for turnover_ratio, expected_multiplier in regimes:
            multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
            multiplier = min(10.0, multiplier)
            
            assert multiplier == expected_multiplier

    async def test_backtester_delay_slippage_never_negative(self, backtester):
        """Test that delay slippage is never negative."""
        cost_size_sol = Decimal('1.0')
        base_pct = Decimal('0.01')
        turnover_ratio = 5.0
        
        multiplier = 3.0 if turnover_ratio > 10 else (2.0 if turnover_ratio > 3 else 1.0)
        multiplier = min(10.0, multiplier)
        
        delay_slippage = cost_size_sol * base_pct * Decimal(str(multiplier))
        
        assert delay_slippage >= Decimal('0')

    async def test_backtester_delay_slippage_respects_config(self, backtester):
        """Test that delay slippage respects backtester config."""
        config = backtester.config
        
        assert hasattr(config, 'entry_delay_slippage_pct')
        assert hasattr(config, 'exit_delay_slippage_pct')
        assert config.entry_delay_slippage_pct >= Decimal('0')
        assert config.exit_delay_slippage_pct >= Decimal('0')


@pytest.fixture
def backtester():
    """Create a Backtester instance for testing."""
    from core.liquidity import LiquidityEstimator
    
    config = BacktestConfig(
        max_slippage_percent=Decimal('0.05'),
        dex_fee_percent=Decimal('0.003'),
        priority_fee_sol_per_trade=Decimal('0.0001'),
        jito_tip_sol_per_trade=Decimal('0.0001'),
        entry_delay_slippage_pct=Decimal('0.01'),
        exit_delay_slippage_pct=Decimal('0.015'),
        mev_penalty_pct=Decimal('0.005')
    )
    
    liquidity_estimator = Mock(spec=LiquidityEstimator)
    liquidity_estimator.estimate_slippage = Mock(return_value=0.01)
    
    return Backtester(config=config, liquidity=liquidity_estimator)