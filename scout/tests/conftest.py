"""
Pytest configuration and fixtures for Scout tests.
"""

import pytest
from core.wqs import WalletMetrics
from core.models import BacktestConfig


@pytest.fixture
def sample_wallet_address():
    """Sample Solana wallet address for testing."""
    return "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"


@pytest.fixture
def high_quality_wallet_metrics(sample_wallet_address):
    """Fixture for a high-quality wallet that should be ACTIVE."""
    return WalletMetrics(
        address=sample_wallet_address,
        roi_7d=15.0,
        roi_30d=45.0,
        trade_count_30d=127,
        win_rate=0.72,
        max_drawdown_30d=8.5,
        win_streak_consistency=0.65,
    )


@pytest.fixture
def medium_quality_wallet_metrics(sample_wallet_address):
    """Fixture for a medium-quality wallet that should be CANDIDATE."""
    return WalletMetrics(
        address=sample_wallet_address,
        roi_7d=5.0,
        roi_30d=15.0,
        trade_count_30d=30,
        win_rate=0.55,
        max_drawdown_30d=15.0,
        win_streak_consistency=0.40,
    )


@pytest.fixture
def low_quality_wallet_metrics(sample_wallet_address):
    """Fixture for a low-quality wallet that should be REJECTED."""
    return WalletMetrics(
        address=sample_wallet_address,
        roi_7d=-5.0,
        roi_30d=-10.0,
        trade_count_30d=5,
        win_rate=0.30,
        max_drawdown_30d=40.0,
        win_streak_consistency=0.10,
    )


@pytest.fixture
def pump_and_dump_wallet_metrics(sample_wallet_address):
    """Fixture for a wallet with pump-and-dump characteristics."""
    return WalletMetrics(
        address=sample_wallet_address,
        roi_7d=200.0,  # Massive recent spike
        roi_30d=50.0,  # 7d ROI > 2x 30d ROI
        trade_count_30d=25,
        win_rate=0.80,
        max_drawdown_30d=5.0,
        win_streak_consistency=0.70,
    )


@pytest.fixture
def low_trade_count_wallet_metrics(sample_wallet_address):
    """Fixture for a wallet with insufficient trade history."""
    return WalletMetrics(
        address=sample_wallet_address,
        roi_7d=20.0,
        roi_30d=40.0,
        trade_count_30d=10,  # < 20 trades
        win_rate=0.75,
        max_drawdown_30d=5.0,
        win_streak_consistency=0.70,
    )


@pytest.fixture
def default_backtest_config():
    """Default backtest configuration matching PDD."""
    return BacktestConfig(
        min_liquidity_shield_usd=10000.0,
        min_liquidity_spear_usd=5000.0,
        dex_fee_percent=0.003,
        max_slippage_percent=0.05,
        min_trades_required=5,
    )


@pytest.fixture
def sample_historical_trade():
    """Sample historical trade for backtest."""
    return {
        "timestamp": "2025-12-01T10:00:00Z",
        "token_address": "BONK111111111111111111111111111111111111111",
        "side": "BUY",
        "amount_sol": 0.5,
        "price": 0.000012,
        "tx_signature": "signature123",
    }


@pytest.fixture
def sample_trades_list(sample_historical_trade):
    """Sample list of historical trades."""
    return [
        sample_historical_trade,
        {
            "timestamp": "2025-12-02T10:00:00Z",
            "token_address": "BONK111111111111111111111111111111111111111",
            "side": "SELL",
            "amount_sol": 0.5,
            "price": 0.000015,
            "tx_signature": "signature456",
        },
        {
            "timestamp": "2025-12-03T10:00:00Z",
            "token_address": "WIF1111111111111111111111111111111111111111",
            "side": "BUY",
            "amount_sol": 0.3,
            "price": 1.25,
            "tx_signature": "signature789",
        },
    ]

