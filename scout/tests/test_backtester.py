"""Tests for backtesting simulator."""

import pytest
from scout.core.backtester import BacktestSimulator, BacktestConfig
from scout.core.liquidity import LiquidityProvider


def test_backtest_simulator_initialization():
    """Test simulator can be initialized."""
    liquidity = LiquidityProvider()
    config = BacktestConfig(
        min_liquidity_shield_usd=10000.0,
        min_liquidity_spear_usd=5000.0,
    )
    
    simulator = BacktestSimulator(liquidity, config)
    
    assert simulator is not None


def test_liquidity_check():
    """Test that trades below liquidity threshold are rejected."""
    # TODO: Implement with mock liquidity data
    pass


def test_slippage_estimation():
    """Test slippage calculation based on trade size vs liquidity."""
    # TODO: Implement slippage calculation tests
    pass


def test_historical_liquidity_validation():
    """Test that historical trades are validated against liquidity at time of trade."""
    # TODO: Implement historical liquidity checks
    pass
