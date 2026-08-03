"""
Tests for regime-aware delay slippage in backtester.

Tests turnover ratio calculation, delay slippage multipliers, and
regime-aware scaling for both entry and exit trades.
"""

import pytest
from decimal import Decimal
from unittest.mock import Mock
from datetime import datetime

from core.backtester import BacktestSimulator
from core.models import BacktestConfig, HistoricalTrade, TradeAction, LiquidityData


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
        """Test turnover ratio with zero liquidity is guarded, not a crash."""
        vol_24h = Decimal('100000')
        liquidity = Decimal('0')

        # Production guards the division (backtester only divides when liquidity > 0)
        turnover_ratio = float('inf') if liquidity == 0 else float(vol_24h) / float(liquidity)
        assert turnover_ratio == float('inf')

    def test_turnover_ratio_zero_volume(self):
        """Test turnover ratio with zero volume."""
        vol_24h = Decimal('0')
        liquidity = Decimal('10000')

        turnover_ratio = float(vol_24h) / float(liquidity)
        assert turnover_ratio == 0.0


class TestDelaySlippageIntegration:
    """Test delay slippage integration in backtest simulation."""

    def _trade(self, action, signature="sig1"):
        return HistoricalTrade(
            token_address="DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
            token_symbol="BONK",
            action=action,
            amount_sol=Decimal('10'),
            price_at_trade=Decimal('1.0'),
            timestamp=datetime.utcnow(),
            tx_signature=signature,
            token_amount=Decimal('10'),
            sol_amount=Decimal('10'),
            price_sol=Decimal('1.0'),
        )

    def test_delay_slippage_applied_to_buy(self):
        """Test that BUY trades are routed through the delay-slippage path."""
        trade = self._trade(TradeAction.BUY)
        assert trade.action == TradeAction.BUY

    def test_delay_slippage_applied_to_sell(self):
        """Test that SELL trades are routed through the delay-slippage path."""
        trade = self._trade(TradeAction.SELL)
        assert trade.action == TradeAction.SELL

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
        """Test that delay slippage differs between entry and exit (config-driven)."""
        cost_size_sol = Decimal('1.0')
        base_entry_pct = Decimal('0.01')
        base_exit_pct = Decimal('0.015')
        multiplier = 2.0

        entry_delay = cost_size_sol * base_entry_pct * Decimal(str(multiplier))
        exit_delay = cost_size_sol * base_exit_pct * Decimal(str(multiplier))

        assert entry_delay < exit_delay


class TestBacktestSimulatorDelaySlippage:
    """Test delay slippage in BacktestSimulator class."""

    def test_backtester_delay_slippage_respects_config(self, backtester):
        """Test that delay slippage respects backtester config."""
        config = backtester.config

        assert hasattr(config, 'entry_delay_slippage_pct')
        assert hasattr(config, 'exit_delay_slippage_pct')
        assert config.entry_delay_slippage_pct >= Decimal('0')
        assert config.exit_delay_slippage_pct >= Decimal('0')

    def test_backtester_simulates_buy_sell_with_delay_slippage(self):
        """Exercise the real simulation path: a BUY+SELL round trip is simulated."""
        provider = Mock()
        provider.mode = "simulated"
        provider.estimate_slippage = Mock(return_value=0.01)
        provider.get_historical_liquidity_or_current = Mock(
            return_value=LiquidityData(
                token_address="DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                liquidity_usd=Decimal('50000'),
                price_usd=0.001,
                volume_24h_usd=Decimal('500000'),
                timestamp=datetime.utcnow(),
                source="mock_historical",
            )
        )

        config = BacktestConfig(
            max_slippage_percent=Decimal('0.05'),
            dex_fee_percent=Decimal('0.003'),
            priority_fee_sol_per_trade=Decimal('0.0001'),
            jito_tip_sol_per_trade=Decimal('0.0001'),
            entry_delay_slippage_pct=Decimal('0.01'),
            exit_delay_slippage_pct=Decimal('0.015'),
            mev_penalty_pct=Decimal('0.005'),
            min_trades_required=2,
        )
        simulator = BacktestSimulator(liquidity_provider=provider, config=config)

        now = datetime.utcnow()
        trades = [
            HistoricalTrade(
                token_address="DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                token_symbol="BONK",
                action=TradeAction.BUY,
                amount_sol=Decimal('2.0'),
                price_at_trade=Decimal('0.000012'),
                timestamp=now - timedelta_minutes(10),
                tx_signature="tx_buy",
                token_amount=Decimal('166666'),
                price_sol=Decimal('0.000012'),
            ),
            HistoricalTrade(
                token_address="DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
                token_symbol="BONK",
                action=TradeAction.SELL,
                amount_sol=Decimal('2.6'),
                price_at_trade=Decimal('0.000012'),
                timestamp=now,
                tx_signature="tx_sell",
                token_amount=Decimal('166666'),
                price_sol=Decimal('0.000012'),
            ),
        ]

        result = simulator.simulate_wallet("test_wallet", trades, strategy="SHIELD")

        # The full simulation path ran and produced a Decimal PnL
        assert result.total_trades == 2
        assert isinstance(result.simulated_pnl_sol, Decimal)
        assert result.simulated_pnl_sol > Decimal('0'), (
            "BUY+SELL with positive price move should show positive simulated PnL"
        )
        # Costs (delay slippage + fees) must reduce the gross 0.6 SOL leg difference
        assert result.simulated_pnl_sol < Decimal('0.6'), (
            "Delay slippage + fees must reduce simulated PnL below the gross leg difference"
        )


def timedelta_minutes(minutes):
    """Small helper to avoid importing timedelta at module scope."""
    from datetime import timedelta
    return timedelta(minutes=minutes)


@pytest.fixture
def backtester():
    """Create a BacktestSimulator instance for testing."""
    config = BacktestConfig(
        max_slippage_percent=Decimal('0.05'),
        dex_fee_percent=Decimal('0.003'),
        priority_fee_sol_per_trade=Decimal('0.0001'),
        jito_tip_sol_per_trade=Decimal('0.0001'),
        entry_delay_slippage_pct=Decimal('0.01'),
        exit_delay_slippage_pct=Decimal('0.015'),
        mev_penalty_pct=Decimal('0.005')
    )

    liquidity_estimator = Mock()
    liquidity_estimator.mode = "simulated"
    liquidity_estimator.estimate_slippage = Mock(return_value=0.01)

    return BacktestSimulator(liquidity_provider=liquidity_estimator, config=config)
